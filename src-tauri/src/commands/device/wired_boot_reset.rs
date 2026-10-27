//! Native S3 USB core resets retain the sampled boot straps. A low sample
//! requires an RTC reset, even after GPIO0 has been released. This is the
//! esptool S3 watchdog-reset sequence, scoped to an already verified image.

pub(crate) const S3_STRAP: u32 = 0x6000_4038;
pub(crate) const S3_GPIO_IN: u32 = 0x6000_403c;
pub(crate) const S3_OPTION1: u32 = 0x6000_812c;
const S3_WDT_CONFIG0: u32 = 0x6000_8098;
const S3_WDT_CONFIG1: u32 = 0x6000_809c;
const S3_WDT_PROTECT: u32 = 0x6000_80b0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegisterOperation {
    Read(u32),
    Write { address: u32, value: u32, mask: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BootReset {
    UsbCore,
    RtcSystem,
}

pub(crate) fn prepare_native_s3_reset(
    mut io: impl FnMut(RegisterOperation) -> Result<u32, String>,
) -> Result<BootReset, String> {
    let strap = io(RegisterOperation::Read(S3_STRAP))?;
    let force = io(RegisterOperation::Read(S3_OPTION1))?;
    let reset = if strap & (1 << 3) == 0 {
        if io(RegisterOperation::Read(S3_GPIO_IN))? & 1 == 0 {
            return Err("Firmware verified, but BOOT/GPIO0 is still low. Release BOOT before restarting the device.".into());
        }
        BootReset::RtcSystem
    } else {
        BootReset::UsbCore
    };
    if force & 1 != 0 {
        io(RegisterOperation::Write { address: S3_OPTION1, value: 0, mask: 1 })?;
    }
    Ok(reset)
}

/// Before arming, every write must be acknowledged. Once armed, USB can
/// disappear before the final response/timeout restoration. The caller must
/// prove application readiness on the same USB identity, including that case.
pub(crate) fn request_rtc_system_reset(
    mut io: impl FnMut(RegisterOperation) -> Result<u32, String>,
) -> Result<Option<String>, String> {
    io(RegisterOperation::Write { address: S3_WDT_PROTECT, value: 0x50d8_3aa1, mask: u32::MAX })?;
    if let Err(error) = io(RegisterOperation::Write { address: S3_WDT_CONFIG1, value: 2000, mask: u32::MAX }) {
        let _ = io(RegisterOperation::Write { address: S3_WDT_PROTECT, value: 0, mask: u32::MAX });
        return Err(error);
    }
    if let Err(error) = io(RegisterOperation::Write {
        address: S3_WDT_CONFIG0, value: (1 << 31) | (5 << 28) | (1 << 8) | 2, mask: u32::MAX,
    }) {
        return Ok(Some(error));
    }
    Ok(io(RegisterOperation::Write { address: S3_WDT_PROTECT, value: 0, mask: u32::MAX }).err())
}

pub(crate) fn application_boot_receipt(line: &str) -> bool {
    line.contains("boot_safety: USB STATUS reset_reason=")
        && line.split_whitespace().any(|part| part == "safe_mode=0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_boot_only_clears_the_force_download_bit() {
        let mut writes = Vec::new();
        assert_eq!(prepare_native_s3_reset(|op| match op {
            RegisterOperation::Read(S3_STRAP) => Ok(8),
            RegisterOperation::Read(S3_OPTION1) => Ok(0x81),
            RegisterOperation::Write { address, value, mask } => { writes.push((address, value, mask)); Ok(0) }
            _ => panic!("unexpected operation {op:?}"),
        }).unwrap(), BootReset::UsbCore);
        assert_eq!(writes, [(S3_OPTION1, 0, 1)]);
    }

    #[test]
    fn released_boot_with_low_latched_strap_needs_a_full_reset() {
        assert_eq!(prepare_native_s3_reset(|op| match op {
            RegisterOperation::Read(address) if address == S3_STRAP || address == S3_OPTION1 => Ok(0),
            RegisterOperation::Read(S3_GPIO_IN) => Ok(1),
            _ => panic!("unexpected operation {op:?}"),
        }).unwrap(), BootReset::RtcSystem);
    }

    #[test]
    fn held_boot_and_failed_reads_never_write_reset_registers() {
        let mut writes = 0;
        assert!(prepare_native_s3_reset(|op| match op {
            RegisterOperation::Read(_) => Ok(0),
            RegisterOperation::Write { .. } => { writes += 1; Ok(0) }
        }).unwrap_err().contains("still low"));
        assert_eq!(writes, 0);
        assert!(prepare_native_s3_reset(|_| Err("read failed".into())).is_err());
    }

    #[test]
    fn rtc_reset_follows_official_sequence_and_stops_before_arming_on_failure() {
        let mut writes = Vec::new();
        assert_eq!(request_rtc_system_reset(|op| { writes.push(op); Ok(0) }).unwrap(), None);
        assert_eq!(writes, [
            RegisterOperation::Write { address: S3_WDT_PROTECT, value: 0x50d8_3aa1, mask: u32::MAX },
            RegisterOperation::Write { address: S3_WDT_CONFIG1, value: 2000, mask: u32::MAX },
            RegisterOperation::Write { address: S3_WDT_CONFIG0, value: 0xd000_0102, mask: u32::MAX },
            RegisterOperation::Write { address: S3_WDT_PROTECT, value: 0, mask: u32::MAX },
        ]);
        let mut attempts = 0;
        assert!(request_rtc_system_reset(|_| { attempts += 1; Err("pre-arm failure".into()) }).is_err());
        assert_eq!(attempts, 1);
    }

    #[test]
    fn usb_loss_after_arm_remains_unconfirmed_until_application_receipt() {
        let mut attempts = 0;
        let uncertain = request_rtc_system_reset(|_| {
            attempts += 1;
            if attempts == 3 { Err("USB disappeared".into()) } else { Ok(0) }
        }).unwrap();
        assert_eq!(uncertain.as_deref(), Some("USB disappeared"));
        assert!(!application_boot_receipt("waiting for download"));
        assert!(!application_boot_receipt("I (100) boot_safety: USB STATUS reset_reason=power_on(1) safe_mode=1"));
        assert!(application_boot_receipt("I (100) boot_safety: USB STATUS reset_reason=power_on(1) safe_mode=0"));
    }
}
