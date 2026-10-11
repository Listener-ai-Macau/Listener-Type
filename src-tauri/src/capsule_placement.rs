//! Presentation-only capsule placement. User moves and automatic layout have
//! separate ownership so status updates cannot undo a drag or overwrite it on disk.
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, PhysicalPosition, Runtime, WebviewWindow};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedPosition {
    monitor_name: Option<String>,
    monitor_x: i32,
    monitor_y: i32,
    // Fractions of the work area: center X and bottom Y of the host. Keeping
    // the bottom anchored lets a translation badge grow upward without a jump.
    center_x: f64,
    bottom_y: f64,
}

#[derive(Clone, Copy)]
struct DragOrigin {
    cursor_x: f64,
    cursor_y: f64,
    window_x: i32,
    window_y: i32,
}

#[derive(Default)]
struct PlacementState {
    saved: Option<SavedPosition>,
    drag: Option<DragOrigin>,
}

pub(crate) struct CapsulePlacement {
    path: Option<PathBuf>,
    state: Arc<Mutex<PlacementState>>,
}

impl CapsulePlacement {
    pub(crate) fn new() -> Self {
        let path = crate::persistence::capsule_placement_path()
            .map_err(|error| log::warn!("[capsule] position storage unavailable: {error}"))
            .ok();
        Self::from_path(path)
    }

    fn from_path(path: Option<PathBuf>) -> Self {
        let saved = path.as_ref().and_then(|path| {
            if !path.exists() {
                return None;
            }
            match std::fs::read(path)
                .map_err(|error| error.to_string())
                .and_then(|bytes| {
                    serde_json::from_slice::<Option<SavedPosition>>(&bytes)
                        .map_err(|error| error.to_string())
                }) {
                Ok(saved) => saved.filter(|p| {
                    p.center_x.is_finite()
                        && p.bottom_y.is_finite()
                        && (0.0..=1.0).contains(&p.center_x)
                        && (0.0..=1.0).contains(&p.bottom_y)
                }),
                Err(error) => {
                    log::warn!("[capsule] saved position unavailable: {error}");
                    None
                }
            }
        });
        Self {
            path,
            state: Arc::new(Mutex::new(PlacementState {
                saved,
                ..Default::default()
            })),
        }
    }

    pub(crate) fn target_monitor<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        window: &WebviewWindow<R>,
    ) -> Option<tauri::Monitor> {
        let saved = self.state.lock().saved.clone();
        if let Some(saved) = saved {
            if let Ok(monitors) = app.available_monitors() {
                if let Some(monitor) = monitors
                    .iter()
                    .find(|monitor| {
                        saved.monitor_name.is_some()
                            && monitor.name() == saved.monitor_name.as_ref()
                    })
                    .or_else(|| {
                        monitors.iter().find(|monitor| {
                            monitor.position().x == saved.monitor_x
                                && monitor.position().y == saved.monitor_y
                        })
                    })
                {
                    return Some(monitor.clone());
                }
            }
            if let Ok(Some(monitor)) = window.current_monitor() {
                return Some(monitor);
            }
        }
        crate::capsule_target_monitor(app, window)
    }

    pub(crate) fn place<R: Runtime>(
        &self,
        window: &WebviewWindow<R>,
        monitor: &tauri::Monitor,
        translation: bool,
    ) -> tauri::Result<()> {
        let bounds = crate::capsule_window_bounds(translation);
        let saved = {
            let state = self.state.lock();
            if state.drag.is_some() {
                return Ok(());
            }
            state.saved.clone()
        };
        let work = monitor.work_area();
        let host = (
            (bounds.width * monitor.scale_factor()).round() as u32,
            (bounds.height * monitor.scale_factor()).round() as u32,
        );
        let work_rect = (
            work.position.x,
            work.position.y,
            work.size.width,
            work.size.height,
        );
        let position = if let Some(saved) = saved {
            restored_position(work_rect, host, saved.center_x, saved.bottom_y)
        } else {
            let default = crate::capsule_bottom_center_physical_position(
                monitor.position().x,
                monitor.position().y,
                monitor.size().width,
                monitor.size().height,
                monitor.scale_factor(),
                bounds,
                crate::capsule_visual_height(translation),
            );
            clamp_position(work_rect, host, default)
        };
        window.set_size(tauri::LogicalSize::new(bounds.width, bounds.height))?;
        window.set_position(PhysicalPosition::new(position.0, position.1))
    }

    pub(crate) fn begin_drag<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        window: &WebviewWindow<R>,
    ) -> Result<(), String> {
        let cursor = app.cursor_position().map_err(|error| error.to_string())?;
        let position = window.outer_position().map_err(|error| error.to_string())?;
        self.state.lock().drag = Some(DragOrigin {
            cursor_x: cursor.x,
            cursor_y: cursor.y,
            window_x: position.x,
            window_y: position.y,
        });
        Ok(())
    }

    pub(crate) fn drag_to_cursor<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        window: &WebviewWindow<R>,
    ) -> Result<(), String> {
        let Some(origin) = self.state.lock().drag else {
            return Ok(());
        };
        let cursor = app.cursor_position().map_err(|error| error.to_string())?;
        // Both values are physical desktop coordinates. JavaScript screenX/Y
        // are deliberately unused: their units change across mixed-DPI displays.
        window
            .set_position(PhysicalPosition::new(
                (origin.window_x as f64 + cursor.x - origin.cursor_x).round() as i32,
                (origin.window_y as f64 + cursor.y - origin.cursor_y).round() as i32,
            ))
            .map_err(|error| error.to_string())
    }

    pub(crate) fn finish_drag<R: Runtime>(&self, window: &WebviewWindow<R>) -> Result<(), String> {
        let mut state = self.state.lock();
        let Some(origin) = state.drag.take() else {
            return Ok(());
        };
        let position = window.outer_position().map_err(|error| error.to_string())?;
        if position.x == origin.window_x && position.y == origin.window_y {
            return Ok(()); // clicking the preview alone does not create a new placement
        }
        let monitor = window
            .current_monitor()
            .map_err(|error| error.to_string())?
            .ok_or("Capsule monitor unavailable")?;
        let size = window.outer_size().map_err(|error| error.to_string())?;
        let work = monitor.work_area();
        if work.size.width == 0 || work.size.height == 0 {
            return Err("Capsule work area unavailable".into());
        }
        state.saved = Some(SavedPosition {
            monitor_name: monitor.name().cloned(),
            monitor_x: monitor.position().x,
            monitor_y: monitor.position().y,
            center_x: ((position.x as f64 + size.width as f64 / 2.0 - work.position.x as f64)
                / work.size.width as f64)
                .clamp(0.0, 1.0),
            bottom_y: ((position.y as f64 + size.height as f64 - work.position.y as f64)
                / work.size.height as f64)
                .clamp(0.0, 1.0),
        });
        let path = self
            .path
            .as_ref()
            .ok_or("Capsule position storage unavailable")?;
        save_position(path, &state.saved)
    }

    pub(crate) fn reset(&self) -> Result<(), String> {
        let mut state = self.state.lock();
        let path = self
            .path
            .as_ref()
            .ok_or("Capsule position storage unavailable")?;
        save_position(path, &None)?;
        state.saved = None;
        state.drag = None;
        Ok(())
    }
}

fn save_position(path: &std::path::Path, saved: &Option<SavedPosition>) -> Result<(), String> {
    let bytes = serde_json::to_vec(saved).map_err(|error| error.to_string())?;
    crate::persistence::write_capsule_placement(path, &bytes).map_err(|error| error.to_string())
}

fn clamp_position(
    work: (i32, i32, u32, u32),
    host: (u32, u32),
    position: (i32, i32),
) -> (i32, i32) {
    let max_x = work.0.saturating_add(work.2.saturating_sub(host.0) as i32);
    let max_y = work.1.saturating_add(work.3.saturating_sub(host.1) as i32);
    (
        position.0.clamp(work.0, max_x),
        position.1.clamp(work.1, max_y),
    )
}

fn restored_position(
    work: (i32, i32, u32, u32),
    host: (u32, u32),
    center_x: f64,
    bottom_y: f64,
) -> (i32, i32) {
    clamp_position(
        work,
        host,
        (
            (work.0 as f64 + center_x * work.2 as f64 - host.0 as f64 / 2.0).round() as i32,
            (work.1 as f64 + bottom_y * work.3 as f64 - host.1 as f64).round() as i32,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_preserves_anchor_across_badge_growth_and_mixed_dpi_negative_monitor() {
        let work = (-2560, -200, 2560, 1400);
        let normal = restored_position(work, (450, 126), 0.6, 0.7);
        let badge = restored_position(work, (450, 177), 0.6, 0.7);
        assert_eq!(normal.0, badge.0);
        assert_eq!(normal.1 + 126, badge.1 + 177);
        let smaller = restored_position((0, 0, 1280, 680), (300, 84), 0.6, 0.7);
        assert_eq!(smaller, (618, 392));
    }

    #[test]
    fn placement_fits_work_area_after_monitor_removal_or_resize() {
        assert_eq!(
            restored_position((0, 40, 800, 560), (300, 118), 1.0, 1.0),
            (500, 482)
        );
        assert_eq!(
            restored_position((-800, 0, 800, 560), (300, 118), 0.0, 0.0),
            (-800, 0)
        );
        assert_eq!(
            restored_position((0, 0, 200, 60), (300, 118), 0.9, 0.9),
            (0, 0)
        );
    }

    #[test]
    fn placement_reload_and_reset_do_not_modify_recording_preferences() {
        let dir = std::env::temp_dir().join(format!("capsule-position-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("capsule-position.json");
        let prefs_path = dir.join("preferences.json");
        let prefs = br#"{"showCapsule":false,"activeAsrProvider":"volcengine"}"#;
        std::fs::write(&prefs_path, prefs).unwrap();
        let position = SavedPosition {
            monitor_name: Some("display-2".into()),
            monitor_x: -1920,
            monitor_y: 0,
            center_x: 0.3,
            bottom_y: 0.8,
        };
        save_position(&path, &Some(position)).unwrap();
        let store = CapsulePlacement::from_path(Some(path.clone()));
        assert_eq!(store.state.lock().saved.as_ref().unwrap().center_x, 0.3);
        store.reset().unwrap();
        assert!(CapsulePlacement::from_path(Some(path.clone()))
            .state
            .lock()
            .saved
            .is_none());
        assert_eq!(std::fs::read(&prefs_path).unwrap(), prefs);
        std::fs::write(&path, br#"{"centerX": 9}"#).unwrap();
        assert!(CapsulePlacement::from_path(Some(path.clone()))
            .state
            .lock()
            .saved
            .is_none());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(prefs_path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
