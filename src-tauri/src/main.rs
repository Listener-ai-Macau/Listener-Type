// Prevents additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    {
        let mut args = std::env::args_os().skip(1);
        let command = args.next();
        if command.as_deref() == Some(std::ffi::OsStr::new("--diagnostic-target-speaker-filter")) {
            let enrollment = args.next();
            let mixture = args.next();
            let output = args.next();
            if let (Some(enrollment), Some(mixture), Some(output), None) =
                (enrollment, mixture, output, args.next())
            {
                let result = listener_type_lib::run_target_speaker_filter_diagnostic(
                    std::path::Path::new(&enrollment),
                    std::path::Path::new(&mixture),
                    std::path::Path::new(&output),
                );
                std::process::exit(if result.is_ok() { 0 } else { 2 });
            }
            std::process::exit(64);
        }
        if command.as_deref() == Some(std::ffi::OsStr::new("--diagnostic-enrolled-wake-recovery")) {
            let input_wav = args.next();
            let phrase = args.next();
            let extracted_wav = args.next();
            let report = args.next();
            if let (Some(input_wav), Some(phrase), Some(extracted_wav), Some(report), None) =
                (input_wav, phrase, extracted_wav, report, args.next())
            {
                let result = listener_type_lib::run_enrolled_wake_recovery_diagnostic(
                    std::path::Path::new(&input_wav),
                    &phrase.to_string_lossy(),
                    std::path::Path::new(&extracted_wav),
                    std::path::Path::new(&report),
                );
                std::process::exit(if result.is_ok() { 0 } else { 2 });
            }
            std::process::exit(64);
        }
    }
    listener_type_lib::run();
}
