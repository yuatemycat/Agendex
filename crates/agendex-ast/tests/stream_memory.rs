use agendex_ast::AstPatcher;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};

#[cfg(windows)]
fn get_working_set_bytes() -> Option<usize> {
    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(
            process: isize,
            counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }

    unsafe {
        let mut counters: ProcessMemoryCounters = std::mem::zeroed();
        counters.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        let handle = GetCurrentProcess();
        if K32GetProcessMemoryInfo(handle, &mut counters, counters.cb) != 0 {
            Some(counters.working_set_size)
        } else {
            None
        }
    }
}

#[cfg(not(windows))]
fn get_working_set_bytes() -> Option<usize> {
    None
}

#[test]
fn test_large_file_streaming_memory_and_byte_identity() {
    let temp_dir = std::env::temp_dir().join(format!("agendex_stream_test_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temporary directory");
    let test_file = temp_dir.join("large_stream.log");

    // Generate a ~32 MiB file in chunks
    let target_size = 32 * 1024 * 1024; // 32 MiB
    let line_content = "ENTRY [2026-10-29 08:00:00] Standard telemetry status log record normal\n";
    let line_len = line_content.len();
    let total_lines = target_size / line_len;
    let target_line_idx = total_lines / 2;

    {
        let file = File::create(&test_file).expect("Failed to create large file");
        let mut writer = BufWriter::with_capacity(128 * 1024, file);

        for i in 0..total_lines {
            if i == target_line_idx {
                writer
                    .write_all(
                        b"ENTRY [2026-10-29 08:00:00] TARGET_MARKER status log record special\n",
                    )
                    .unwrap();
            } else {
                writer.write_all(line_content.as_bytes()).unwrap();
            }
        }
        writer.flush().unwrap();
    }

    let initial_metadata = std::fs::metadata(&test_file).unwrap();
    assert!(
        initial_metadata.len() >= 30 * 1024 * 1024,
        "File must be >= 30 MiB"
    );

    let patcher = AstPatcher::new();

    // Perform streaming regex replacement
    let result = patcher
        .stream_replace_regex_file(&test_file, "TARGET_MARKER", "REPLACED_MARKER")
        .expect("Streaming replacement must succeed");

    assert_eq!(result.total_replacements, 1);
    assert_eq!(result.lines_modified, 1);
    assert_eq!(result.lines_processed, total_lines);

    // Verify memory RSS constraint
    if let Some(rss) = get_working_set_bytes() {
        let rss_mb = rss / (1024 * 1024);
        assert!(
            rss_mb < 64,
            "Process RSS must remain under 64 MiB during streaming, got {} MiB",
            rss_mb
        );
    }

    // Verify untouched lines are byte-identical and target was replaced
    {
        let file = File::open(&test_file).expect("Failed to reopen patched file");
        let reader = BufReader::with_capacity(128 * 1024, file);

        for (i, line_res) in reader.lines().enumerate() {
            let line = line_res.unwrap();
            if i == target_line_idx {
                assert!(line.contains("REPLACED_MARKER"));
                assert!(!line.contains("TARGET_MARKER"));
            } else {
                assert_eq!(
                    line,
                    "ENTRY [2026-10-29 08:00:00] Standard telemetry status log record normal"
                );
            }
        }
    }

    let _ = std::fs::remove_dir_all(temp_dir);
}
