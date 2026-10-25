//! Retain the latest page request until the main renderer has registered.
use crate::types::DeviceCustomKeyAppPage;
use parking_lot::Mutex;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "page", rename_all = "camelCase")]
pub(crate) enum MainWindowDestination {
    Providers,
    DevicePage(DeviceCustomKeyAppPage),
}

#[derive(Default)]
struct PendingNavigation {
    destination: Option<MainWindowDestination>,
}

impl PendingNavigation {
    fn request(&mut self, destination: MainWindowDestination) {
        self.destination = Some(destination);
    }
    fn take(&mut self) -> Option<MainWindowDestination> {
        self.destination.take()
    }
}

static PENDING: Mutex<PendingNavigation> = Mutex::new(PendingNavigation { destination: None });

pub(crate) fn request(destination: MainWindowDestination) {
    PENDING.lock().request(destination);
}

pub(crate) fn take() -> Option<MainWindowDestination> {
    PENDING.lock().take()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_renderer_takes_the_latest_request_once() {
        let mut pending = PendingNavigation::default();
        pending.request(MainWindowDestination::Providers);
        pending.request(MainWindowDestination::DevicePage(DeviceCustomKeyAppPage::History));
        assert_eq!(pending.take(), Some(MainWindowDestination::DevicePage(DeviceCustomKeyAppPage::History)));
        assert_eq!(pending.take(), None);
        pending.request(MainWindowDestination::Providers);
        assert_eq!(pending.take(), Some(MainWindowDestination::Providers));
    }
}
