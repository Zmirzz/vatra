//! Windows hardware, from the registry and system calls. WMI isn't used: its `AdapterRAM` stops at 4 GiB.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_ANY, RegGetValueW};
use windows_sys::Win32::System::SystemInformation::{
    GetLogicalProcessorInformationEx, GlobalMemoryStatusEx, MEMORYSTATUSEX, RelationProcessorCore,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};

use super::{Cpu, Gpu, GpuSource, HardwareReport, Ram, cpu_features};

const DISPLAY_CLASS: &str = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

pub(super) fn detect(models_dir: Option<&Path>) -> HardwareReport {
    HardwareReport {
        cpu: cpu_features(Cpu {
            name: registry(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString")
                .and_then(|bytes| text(&bytes))
                .map(|name| name.trim().to_string()),
            cores: cores(),
            threads: std::thread::available_parallelism().ok().and_then(|n| u32::try_from(n.get()).ok()),
            ..Cpu::default()
        }),
        ram: ram(),
        gpus: gpus(),
        disk_free: models_dir.and_then(disk_free),
    }
}

fn ram() -> Ram {
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return Ram::default();
    }
    Ram { total: status.ullTotalPhys, available: status.ullAvailPhys }
}

fn cores() -> Option<u32> {
    let mut len = 0u32;
    unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, std::ptr::null_mut(), &mut len) };
    if len == 0 {
        return None;
    }
    // u64 storage keeps the records aligned for the call; they are read back as bytes.
    let mut storage = vec![0u64; (len as usize).div_ceil(8)];
    let info = storage.as_mut_ptr().cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>();
    if unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, info, &mut len) } == 0 {
        return None;
    }
    let buffer: Vec<u8> = storage.iter().flat_map(|word| word.to_ne_bytes()).collect();
    let (mut offset, mut count) = (0usize, 0u32);
    while offset + 8 <= len as usize {
        // Every record starts with its relationship (u32) and its own size (u32).
        let size = u32::from_ne_bytes(buffer[offset + 4..offset + 8].try_into().ok()?) as usize;
        if size == 0 {
            break;
        }
        count += 1;
        offset += size;
    }
    Some(count)
}

/// Display adapters from the registry. `HardwareInformation.qwMemorySize` is 64-bit, unlike WMI.
fn gpus() -> Vec<Gpu> {
    (0..32)
        .filter_map(|index| {
            let key = format!(r"{DISPLAY_CLASS}\{index:04}");
            let name = text(&registry(&key, "DriverDesc")?)?;
            let memory = registry(&key, "HardwareInformation.qwMemorySize")
                .and_then(|bytes| Some(u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?)))
                .or_else(|| {
                    registry(&key, "HardwareInformation.MemorySize")
                        .and_then(|bytes| Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as u64))
                })?;
            Some(Gpu {
                integrated: memory < 512 * 1024 * 1024,
                name: name.trim().to_string(),
                backend: None,
                total: memory,
                free: None,
                source: GpuSource::System,
            })
        })
        .collect()
}

pub(super) fn disk_free(dir: &Path) -> Option<u64> {
    let existing = dir.ancestors().find(|path| path.exists())?;
    let wide = wide(existing.as_os_str());
    let mut free = 0u64;
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(free)
}

fn registry(key: &str, value: &str) -> Option<Vec<u8>> {
    let (key, value) = (wide(key.as_ref()), wide(value.as_ref()));
    let mut len = 0u32;
    let status = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), RRF_RT_ANY, std::ptr::null_mut(), std::ptr::null_mut(), &mut len)
    };
    if status != ERROR_SUCCESS || len == 0 {
        return None;
    }
    let mut buffer = vec![0u8; len as usize];
    let status = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), RRF_RT_ANY, std::ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut len)
    };
    (status == ERROR_SUCCESS).then(|| {
        buffer.truncate(len as usize);
        buffer
    })
}

/// A REG_SZ value as text, without the trailing NULs.
fn text(bytes: &[u8]) -> Option<String> {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    let text = String::from_utf16_lossy(&units).trim_end_matches('\0').to_string();
    (!text.is_empty()).then_some(text)
}

fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}
