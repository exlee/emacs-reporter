// platform.rs
//#![cfg(target_os = "macos")]

use std::collections::HashMap;
use std::ffi::CStr;
use std::path::PathBuf;

use anyhow::Context;

// ── Public types ──────────────────────────────────────────────────────────────

pub struct CpuData {
    pub user_ms: i64,
    pub system_ms: i64,
    pub messages_sent: i64,
    pub messages_received: i64,
    pub syscalls_mach: i64,
    pub syscalls_unix: i64,
    pub context_switches: i64,
}

pub struct MemoryData {
    pub virt_size: i64,
    pub resident_size: i64,
    pub resident_size_peak: i64,
    pub phys_footprint: i64,
    pub phys_footprint_peak: i64,
    pub private_size: Option<i64>,
    pub shared_size: Option<i64>,
    pub swapped_size: Option<i64>,
    pub purgeable_volatile: Option<i64>,
    pub purgeable_nonvolatile: Option<i64>,
}

pub struct VmRegion {
    pub region_type: String,
    pub region_count: i64,
    pub block_count: Option<i64>,
    pub virtual_size: i64,
    pub resident_size: i64,
    pub dirty_size: i64,
    pub swapped_size: i64,
    pub shared_size: Option<i64>,
    pub private_size: Option<i64>,
    pub protection: Option<String>,
    pub share_mode: Option<String>,
}

// ── FFI bindings ──────────────────────────────────────────────────────────────

#[allow(non_camel_case_types)]
mod ffi {
    use std::ffi::c_int;

    pub type pid_t = c_int;
    pub type kern_return_t = i32;
    pub type mach_port_t = u32;
    pub type vm_map_t = mach_port_t;
    pub type mach_vm_address_t = u64;
    pub type mach_vm_size_t = u64;
    pub type vm_region_recurse_info_t = *mut i32;
    pub type natural_t = u32;

    pub const KERN_SUCCESS: kern_return_t = 0;
    pub const VM_REGION_SUBMAP_INFO_COUNT_64: natural_t = 19;

    // share modes
    pub const SM_COW: u8 = 1;
    pub const SM_PRIVATE: u8 = 2;
    pub const SM_EMPTY: u8 = 3;
    pub const SM_SHARED: u8 = 4;
    pub const SM_TRUESHARED: u8 = 5;
    pub const SM_PRIVATE_ALIASED: u8 = 6;
    pub const SM_SHARED_ALIASED: u8 = 7;
    pub const SM_LARGE_PAGE: u8 = 8;

    #[repr(C)]
    #[derive(Default)]
    pub struct vm_region_submap_info_64 {
        pub protection: i32,
        pub max_protection: i32,
        pub inheritance: u32,
        pub offset: u64,
        pub user_tag: u32,
        pub pages_resident: u32,
        pub pages_shared_now_private: u32,
        pub pages_swapped_out: u32,
        pub pages_dirtied: u32,
        pub ref_count: u32,
        pub shadow_depth: u16,
        pub external_pager: u8,
        pub share_mode: u8,
        pub is_submap: i32,
        pub behavior: i32,
        pub object_id: u32,
        pub user_wired_count: u16,
        pub pages_reusable: u32,
    }

    #[repr(C)]
    pub struct proc_taskinfo {
        pub pti_virtual_size: u64,
        pub pti_resident_size: u64,
        pub pti_total_user: u64,
        pub pti_total_system: u64,
        pub pti_threads_user: u64,
        pub pti_threads_system: u64,
        pub pti_policy: i32,
        pub pti_faults: i32,
        pub pti_pageins: i32,
        pub pti_cow_faults: i32,
        pub pti_messages_sent: i32,
        pub pti_messages_received: i32,
        pub pti_syscalls_mach: i32,
        pub pti_syscalls_unix: i32,
        pub pti_csw: i32,
        pub pti_threadnum: i32,
        pub pti_numrunning: i32,
        pub pti_priority: i32,
    }

    #[repr(C)]
    pub struct task_vm_info {
        pub virtual_size: u64,
        pub region_count: i32,
        pub page_size: i32,
        pub resident_size: u64,
        pub resident_size_peak: u64,
        pub device: u64,
        pub device_peak: u64,
        pub internal: u64,
        pub internal_peak: u64,
        pub external: u64,
        pub external_peak: u64,
        pub reusable: u64,
        pub reusable_peak: u64,
        pub purgeable_volatile_pmap: u64,
        pub purgeable_volatile_resident: u64,
        pub purgeable_volatile_virtual: u64,
        pub compressed: u64,
        pub compressed_peak: u64,
        pub compressed_lifetime: u64,
        pub phys_footprint: u64,
        pub min_address: u64,
        pub max_address: u64,
        pub ledger_phys_footprint_peak: u64,
        pub ledger_purgeable_nonvolatile: u64,
        pub ledger_purgeable_novolatile_compressed: u64,
        pub ledger_purgeable_volatile: u64,
        pub ledger_purgeable_volatile_compressed: u64,
        pub ledger_tag_network_nonvolatile: u64,
        pub ledger_tag_network_nonvolatile_compressed: u64,
        pub ledger_tag_network_volatile: u64,
        pub ledger_tag_network_volatile_compressed: u64,
        pub ledger_tag_media_footprint: u64,
        pub ledger_tag_media_footprint_compressed: u64,
        pub ledger_tag_media_nofootprint: u64,
        pub ledger_tag_media_nofootprint_compressed: u64,
        pub ledger_tag_graphics_footprint: u64,
        pub ledger_tag_graphics_footprint_compressed: u64,
        pub ledger_tag_graphics_nofootprint: u64,
        pub ledger_tag_graphics_nofootprint_compressed: u64,
        pub ledger_tag_neural_footprint: u64,
        pub ledger_tag_neural_footprint_compressed: u64,
        pub ledger_tag_neural_nofootprint: u64,
        pub ledger_tag_neural_nofootprint_compressed: u64,
        pub limit_bytes_remaining: u64,
        pub decompressions: i32,
        pub ledger_swapins: u64,
    }

    pub const TASK_VM_INFO: u32 = 22;
    pub const TASK_VM_INFO_COUNT: u32 =
        (std::mem::size_of::<task_vm_info>() / std::mem::size_of::<u32>()) as u32;

    pub const PROC_PIDTASKINFO: i32 = 4;

    unsafe extern "C" {

        pub fn task_for_pid(
            target_tport: mach_port_t,
            pid: pid_t,
            t: *mut mach_port_t,
        ) -> kern_return_t;

        pub fn mach_task_self() -> mach_port_t;

        pub fn mach_vm_region_recurse(
            target_task: vm_map_t,
            address: *mut mach_vm_address_t,
            size: *mut mach_vm_size_t,
            depth: *mut natural_t,
            info: vm_region_recurse_info_t,
            info_count: *mut natural_t,
        ) -> kern_return_t;

        pub fn mach_port_deallocate(task: mach_port_t, name: mach_port_t) -> kern_return_t;

        pub fn task_info(
            target_task: mach_port_t,
            flavor: u32,
            task_info_out: *mut i32,
            task_info_out_count: *mut u32,
        ) -> kern_return_t;

        pub fn proc_pidinfo(
            pid: c_int,
            flavor: c_int,
            arg: u64,
            buffer: *mut std::ffi::c_void,
            buffersize: c_int,
        ) -> c_int;

        pub fn proc_listallpids(buffer: *mut std::ffi::c_void, buffersize: c_int) -> c_int;

        pub fn proc_pidpath(pid: c_int, buffer: *mut std::ffi::c_void, buffersize: u32) -> c_int;
    }

    pub const PROC_PIDPATHINFO_MAXSIZE: u32 = 4096;
}

// ── Process discovery ─────────────────────────────────────────────────────────

pub fn find_emacs_pids() -> anyhow::Result<Vec<i32>> {
    let count = unsafe { ffi::proc_listallpids(std::ptr::null_mut(), 0) };
    anyhow::ensure!(count > 0, "proc_listallpids failed");

    let mut pids = vec![0i32; count as usize + 16]; // small headroom
    let filled = unsafe {
        ffi::proc_listallpids(
            pids.as_mut_ptr() as *mut _,
            (pids.len() * std::mem::size_of::<i32>()) as i32,
        )
    };
    anyhow::ensure!(filled > 0, "proc_listallpids returned no pids");

    pids.truncate(filled as usize);

    let mut emacs_pids = Vec::new();
    for pid in pids {
        if pid <= 0 {
            continue;
        }
        if let Ok(path) = process_binary_path(pid) {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.eq_ignore_ascii_case("emacs") {
                emacs_pids.push(pid);
            }
        }
    }

    Ok(emacs_pids)
}

// ── Process metadata ──────────────────────────────────────────────────────────

pub fn process_binary_path(pid: i32) -> anyhow::Result<PathBuf> {
    let mut buf = vec![0u8; ffi::PROC_PIDPATHINFO_MAXSIZE as usize];
    let ret = unsafe {
        ffi::proc_pidpath(
            pid,
            buf.as_mut_ptr() as *mut _,
            ffi::PROC_PIDPATHINFO_MAXSIZE,
        )
    };
    anyhow::ensure!(ret > 0, "proc_pidpath failed for pid {pid}");
    let cstr = CStr::from_bytes_until_nul(&buf).context("proc_pidpath: invalid path")?;
    Ok(PathBuf::from(cstr.to_string_lossy().as_ref()))
}

pub fn process_start_time(pid: i32) -> anyhow::Result<i64> {
    let out = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .context("ps failed")?;

    anyhow::ensure!(out.status.success(), "ps returned non-zero for pid {pid}");

    let s = String::from_utf8(out.stdout)
        .context("ps output not utf8")?;
    let s = s.trim();

    // lstart format: "Sat Mar 21 17:12:57 2026"
    let dt = chrono::NaiveDateTime::parse_from_str(s, "%a %b %e %H:%M:%S %Y")
        .context("failed to parse lstart: {s}")?;

    Ok(dt.and_utc().timestamp())
}

// ── CPU ───────────────────────────────────────────────────────────────────────

pub fn collect_cpu(pid: i32) -> anyhow::Result<CpuData> {
    let mut info: ffi::proc_taskinfo = unsafe { std::mem::zeroed() };
    let ret = unsafe {
        ffi::proc_pidinfo(
            pid,
            ffi::PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut _,
            std::mem::size_of::<ffi::proc_taskinfo>() as i32,
        )
    };
    anyhow::ensure!(
        ret == std::mem::size_of::<ffi::proc_taskinfo>() as i32,
        "proc_pidinfo PROC_PIDTASKINFO failed for pid {pid}"
    );

    // kernel reports in nanoseconds
    Ok(CpuData {
        user_ms: (info.pti_total_user / 1_000_000) as i64,
        system_ms: (info.pti_total_system / 1_000_000) as i64,
        messages_sent: info.pti_messages_sent as i64,
        messages_received: info.pti_messages_received as i64,
        syscalls_mach: info.pti_syscalls_mach as i64,
        syscalls_unix: info.pti_syscalls_unix as i64,
        context_switches: info.pti_csw as i64,
    })
}

// ── Memory ────────────────────────────────────────────────────────────────────

pub fn collect_memory(pid: i32) -> anyhow::Result<MemoryData> {
    let task = task_for_pid(pid)?;
    let _guard = TaskGuard(task);

    let mut info: ffi::task_vm_info = unsafe { std::mem::zeroed() };
    let mut count = ffi::TASK_VM_INFO_COUNT;

    let kr = unsafe {
        ffi::task_info(
            task,
            ffi::TASK_VM_INFO,
            &mut info as *mut _ as *mut i32,
            &mut count,
        )
    };
    anyhow::ensure!(
        kr == ffi::KERN_SUCCESS,
        "task_info TASK_VM_INFO failed for pid {pid}: {kr}"
    );

    // Also grab resident_size_peak from proc_taskinfo
    let mut ptask: ffi::proc_taskinfo = unsafe { std::mem::zeroed() };
    unsafe {
        ffi::proc_pidinfo(
            pid,
            ffi::PROC_PIDTASKINFO,
            0,
            &mut ptask as *mut _ as *mut _,
            std::mem::size_of::<ffi::proc_taskinfo>() as i32,
        );
    };

    Ok(MemoryData {
        virt_size: info.virtual_size as i64,
        resident_size: info.resident_size as i64,
        resident_size_peak: ptask.pti_resident_size as i64, // best available
        phys_footprint: info.phys_footprint as i64,
        phys_footprint_peak: info.ledger_phys_footprint_peak as i64,
        private_size: Some(info.internal as i64),
        shared_size: Some(info.external as i64),
        swapped_size: Some(info.compressed as i64),
        purgeable_volatile: Some(info.purgeable_volatile_resident as i64),
        purgeable_nonvolatile: Some(info.purgeable_volatile_pmap as i64),
    })
}

// ── VM regions ────────────────────────────────────────────────────────────────

pub fn collect_vm_regions(pid: i32) -> anyhow::Result<Vec<VmRegion>> {
    let task = task_for_pid(pid)?;
    let _guard = TaskGuard(task);

    // Aggregate by region type tag
    #[derive(Default)]
    struct Accum {
        region_count: i64,
        virtual_size: i64,
        resident_size: i64,
        dirty_size: i64,
        swapped_size: i64,
        shared_size: i64,
        private_size: i64,
        protection: Option<String>,
        share_mode: Option<String>,
    }

    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as i64;
    let mut map: HashMap<String, Accum> = HashMap::new();

    let mut address: ffi::mach_vm_address_t = 1;
    let mut depth: ffi::natural_t = 1024;

    loop {
        let mut size: ffi::mach_vm_size_t = 0;
        let mut info: ffi::vm_region_submap_info_64 = unsafe { std::mem::zeroed() };
        let mut count = ffi::VM_REGION_SUBMAP_INFO_COUNT_64;

        let kr = unsafe {
            ffi::mach_vm_region_recurse(
                task,
                &mut address,
                &mut size,
                &mut depth,
                &mut info as *mut _ as ffi::vm_region_recurse_info_t,
                &mut count,
            )
        };

        if kr != ffi::KERN_SUCCESS {
            break; // KERN_INVALID_ADDRESS == end of map
        }

        if info.is_submap != 0 {
            depth += 1;
            continue; // descend into submap
        }

        let tag = region_tag(info.user_tag);
        let prot = protection_string(info.protection);
        let mode = share_mode_string(info.share_mode);

        let entry = map.entry(tag).or_default();
        entry.region_count += 1;
        entry.virtual_size += size as i64;
        entry.resident_size += info.pages_resident as i64 * page_size;
        entry.dirty_size +=
            (info.pages_dirtied as i64 + info.pages_shared_now_private as i64) * page_size;
        entry.swapped_size += info.pages_swapped_out as i64 * page_size;

        // shared vs private heuristic from share mode
        match info.share_mode {
            ffi::SM_PRIVATE | ffi::SM_PRIVATE_ALIASED => {
                entry.private_size += size as i64;
            }
            ffi::SM_SHARED | ffi::SM_TRUESHARED | ffi::SM_SHARED_ALIASED => {
                entry.shared_size += size as i64;
            }
            ffi::SM_COW => {
                // copy-on-write: count as private for our purposes
                entry.private_size += size as i64;
            }
            _ => {}
        }

        // Keep the most common protection/share_mode (first seen is fine for summary)
        if entry.protection.is_none() {
            entry.protection = Some(prot);
        }
        if entry.share_mode.is_none() {
            entry.share_mode = Some(mode);
        }

        address += size;
        depth = 1024; // reset depth for next region
    }

    Ok(map
        .into_iter()
        .map(|(region_type, a)| VmRegion {
            region_type,
            region_count: a.region_count,
            block_count: None, // malloc zone detail not implemented
            virtual_size: a.virtual_size,
            resident_size: a.resident_size,
            dirty_size: a.dirty_size,
            swapped_size: a.swapped_size,
            shared_size: Some(a.shared_size),
            private_size: Some(a.private_size),
            protection: a.protection,
            share_mode: a.share_mode,
        })
        .collect())
}

// ── Mach helpers ──────────────────────────────────────────────────────────────

fn task_for_pid(pid: i32) -> anyhow::Result<ffi::mach_port_t> {
    let mut task: ffi::mach_port_t = 0;
    let kr = unsafe { ffi::task_for_pid(ffi::mach_task_self(), pid, &mut task) };
    anyhow::ensure!(
        kr == ffi::KERN_SUCCESS,
        "task_for_pid failed for pid {pid}: kern_return {kr} \
         (Emacs may not be signed with get-task-allow, or try sudo)"
    );
    Ok(task)
}

// RAII guard — deallocates Mach port on drop
struct TaskGuard(ffi::mach_port_t);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        unsafe { ffi::mach_port_deallocate(ffi::mach_task_self(), self.0) };
    }
}

// ── Region tag → human name ───────────────────────────────────────────────────
// user_tag values from <mach/vm_statistics.h>
include!("region_tag.rs");

fn protection_string(prot: i32) -> String {
    let r = if prot & 1 != 0 { 'r' } else { '-' };
    let w = if prot & 2 != 0 { 'w' } else { '-' };
    let x = if prot & 4 != 0 { 'x' } else { '-' };
    format!("{r}{w}{x}")
}

fn share_mode_string(mode: u8) -> String {
    match mode {
        ffi::SM_COW => "COW",
        ffi::SM_PRIVATE => "PRV",
        ffi::SM_EMPTY => "NUL",
        ffi::SM_SHARED => "SHM",
        ffi::SM_TRUESHARED => "TSH",
        ffi::SM_PRIVATE_ALIASED => "PAL",
        ffi::SM_SHARED_ALIASED => "SAL",
        ffi::SM_LARGE_PAGE => "LPG",
        _ => "UNK",
    }
    .to_owned()
}

// ── Meta helpers ──────────────────────────────────────────────────────────────

pub fn os_version() -> String {
    std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned())
}

pub fn generate_user_uuid() -> String {
    // Random UUID v4 — no PII, stable for the lifetime of the db

    uuid::Uuid::new_v4().to_string()
}
