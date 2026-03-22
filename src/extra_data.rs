#![cfg(target_os = "macos")]

// ── Public types ──────────────────────────────────────────────────────────────

pub struct ThreadData {
    pub thread_count: i64,
    pub running_count: i64,
    pub faults_total: i64,
    pub faults_cow: i64,
    pub pageins: i64,
}

pub struct IoData {
    pub bytes_read: i64,
    pub bytes_written: i64,
    pub logical_writes: i64,
}

pub struct PortData {
    pub mach_port_count: i64,
    pub fd_count: i64,
}

pub struct EnergyData {
    pub cpu_energy_nj: i64,
    pub gpu_time_ms: i64,
}

// ── FFI ───────────────────────────────────────────────────────────────────────

#[allow(non_camel_case_types)]
mod ffi {
    use std::ffi::c_int;

    pub type mach_port_t = u32;
    pub type kern_return_t = i32;
    pub type pid_t = c_int;

    pub const KERN_SUCCESS: kern_return_t = 0;
    pub const TASK_BASIC_INFO_64: u32 = 5;
    pub const TASK_BASIC_INFO_64_COUNT: u32 =
        (std::mem::size_of::<task_basic_info_64>() / std::mem::size_of::<u32>()) as u32;

    pub const PROC_PIDTASKINFO: i32 = 4;
    pub const PROC_PIDLISTFDS: i32 = 1;

    #[repr(C)]
    pub struct task_basic_info_64 {
        pub suspend_count: i32,
        pub virtual_size: u64,
        pub resident_size: u64,
        pub user_time_seconds: u32,
        pub user_time_microseconds: u32,
        pub system_time_seconds: u32,
        pub system_time_microseconds: u32,
        pub policy: i32,
    }

    // matches struct proc_taskinfo in <sys/proc_info.h>
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

    // matches struct rusage_info_v2 in <sys/resource.h>
    // we only need the first few fields
    #[repr(C)]
    pub struct rusage_info_v2 {
        pub ri_uuid: [u8; 16],
        pub ri_user_time: u64,
        pub ri_system_time: u64,
        pub ri_pkg_idle_wkups: u64,
        pub ri_interrupt_wkups: u64,
        pub ri_pageins: u64,
        pub ri_wired_size: u64,
        pub ri_resident_size: u64,
        pub ri_phys_footprint: u64,
        pub ri_proc_start_abstime: u64,
        pub ri_proc_exit_abstime: u64,
        pub ri_child_user_time: u64,
        pub ri_child_system_time: u64,
        pub ri_child_pkg_idle_wkups: u64,
        pub ri_child_interrupt_wkups: u64,
        pub ri_child_pageins: u64,
        pub ri_child_elapsed_abstime: u64,
        pub ri_diskio_bytesread: u64,
        pub ri_diskio_byteswritten: u64,
        pub ri_cpu_time_qos_default: u64,
        pub ri_cpu_time_qos_maintenance: u64,
        pub ri_cpu_time_qos_background: u64,
        pub ri_cpu_time_qos_utility: u64,
        pub ri_cpu_time_qos_legacy: u64,
        pub ri_cpu_time_qos_user_initiated: u64,
        pub ri_cpu_time_qos_user_interactive: u64,
        pub ri_billed_system_time: u64,
        pub ri_serviced_system_time: u64,
        pub ri_logical_writes: u64,
        pub ri_lifetime_max_phys_footprint: u64,
        pub ri_instructions: u64,
        pub ri_cycles: u64,
        pub ri_billed_energy: u64,
        pub ri_serviced_energy: u64,
        pub ri_interval_max_phys_footprint: u64,
        pub ri_runnable_time: u64,
    }

    // proc_fdinfo is just a type tag + fd number, 8 bytes each
    #[repr(C)]
    pub struct proc_fdinfo {
        pub proc_fd: i32,
        pub proc_fdtype: u32,
    }

    unsafe extern "C" {
        pub fn task_for_pid(
            target_tport: mach_port_t,
            pid: pid_t,
            t: *mut mach_port_t,
        ) -> kern_return_t;

        pub fn mach_task_self() -> mach_port_t;

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

        pub fn proc_pid_rusage(
            pid: pid_t,
            flavor: c_int,
            buffer: *mut std::ffi::c_void,
        ) -> c_int;
    }
}

// ── RAII Mach port guard ──────────────────────────────────────────────────────

struct TaskGuard(ffi::mach_port_t);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        unsafe { ffi::mach_port_deallocate(ffi::mach_task_self(), self.0) };
    }
}

fn task_for_pid(pid: i32) -> anyhow::Result<ffi::mach_port_t> {
    let mut task: ffi::mach_port_t = 0;
    let kr = unsafe { ffi::task_for_pid(ffi::mach_task_self(), pid, &mut task) };
    anyhow::ensure!(
        kr == ffi::KERN_SUCCESS,
        "task_for_pid failed for pid {pid}: kern_return {kr}"
    );
    Ok(task)
}

// ── Thread + fault data ───────────────────────────────────────────────────────

pub fn collect_threads_and_faults(pid: i32) -> anyhow::Result<ThreadData> {
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

    Ok(ThreadData {
        thread_count: info.pti_threadnum as i64,
        running_count: info.pti_numrunning as i64,
        faults_total: info.pti_faults as i64,
        faults_cow: info.pti_cow_faults as i64,
        pageins: info.pti_pageins as i64,
    })
}

// ── I/O data ──────────────────────────────────────────────────────────────────

pub fn collect_io(pid: i32) -> anyhow::Result<IoData> {
    let mut info: ffi::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let ret = unsafe {
        ffi::proc_pid_rusage(
            pid,
            2, // RUSAGE_INFO_V2
            &mut info as *mut _ as *mut _,
        )
    };
    anyhow::ensure!(ret == 0, "proc_pid_rusage failed for pid {pid}: {ret}");

    Ok(IoData {
        bytes_read: info.ri_diskio_bytesread as i64,
        bytes_written: info.ri_diskio_byteswritten as i64,
        logical_writes: info.ri_logical_writes as i64,
    })
}

// ── Port + FD counts ──────────────────────────────────────────────────────────

pub fn collect_ports_and_fds(pid: i32) -> anyhow::Result<PortData> {
    // Mach port count via TASK_BASIC_INFO_64
    let task = task_for_pid(pid)?;
    let _guard = TaskGuard(task);

    // mach port count: ask for the fd table size first with null buffer
    // then count entries
    let fd_count = {
        let needed = unsafe {
            ffi::proc_pidinfo(
                pid,
                ffi::PROC_PIDLISTFDS,
                0,
                std::ptr::null_mut(),
                0,
            )
        };
        if needed <= 0 {
            0i64
        } else {
            let entry_size = std::mem::size_of::<ffi::proc_fdinfo>() as i32;
            let cap = (needed / entry_size + 16) * entry_size; // small headroom
            let mut buf = vec![0u8; cap as usize];
            let filled = unsafe {
                ffi::proc_pidinfo(
                    pid,
                    ffi::PROC_PIDLISTFDS,
                    0,
                    buf.as_mut_ptr() as *mut _,
                    cap,
                )
            };
            if filled <= 0 {
                0i64
            } else {
                (filled / entry_size) as i64
            }
        }
    };

    // Mach port count via task_basic_info_64
    let mut basic: ffi::task_basic_info_64 = unsafe { std::mem::zeroed() };
    let mut count = ffi::TASK_BASIC_INFO_64_COUNT;
    let kr = unsafe {
        ffi::task_info(
            task,
            ffi::TASK_BASIC_INFO_64,
            &mut basic as *mut _ as *mut i32,
            &mut count,
        )
    };

    // TASK_BASIC_INFO_64 doesn't expose port count directly —
    // use TASK_EXTMOD_INFO or fall back to parsing /proc is not available on macOS.
    // Best available without private API: mach_port_names count.
    let mach_port_count = if kr == ffi::KERN_SUCCESS {
        collect_port_count_via_names(task).unwrap_or(0)
    } else {
        0
    };

    Ok(PortData {
        mach_port_count,
        fd_count,
    })
}

fn collect_port_count_via_names(task: ffi::mach_port_t) -> anyhow::Result<i64> {
    #[allow(non_camel_case_types)]
    type mach_port_name_array_t = *mut u32;
    #[allow(non_camel_case_types)]
    type mach_port_type_array_t = *mut u32;

    unsafe extern "C" {
        fn mach_port_names(
            task: ffi::mach_port_t,
            names: *mut mach_port_name_array_t,
            names_count: *mut u32,
            types: *mut mach_port_type_array_t,
            types_count: *mut u32,
        ) -> ffi::kern_return_t;

        fn vm_deallocate(
            target_task: ffi::mach_port_t,
            address: usize,
            size: usize,
        ) -> ffi::kern_return_t;
    }

    let mut names: mach_port_name_array_t = std::ptr::null_mut();
    let mut names_count: u32 = 0;
    let mut types: mach_port_type_array_t = std::ptr::null_mut();
    let mut types_count: u32 = 0;

    let kr = unsafe {
        mach_port_names(task, &mut names, &mut names_count, &mut types, &mut types_count)
    };
    anyhow::ensure!(kr == ffi::KERN_SUCCESS, "mach_port_names failed: {kr}");

    // deallocate the returned arrays
    unsafe {
        vm_deallocate(
            ffi::mach_task_self(),
            names as usize,
            names_count as usize * std::mem::size_of::<u32>(),
        );
        vm_deallocate(
            ffi::mach_task_self(),
            types as usize,
            types_count as usize * std::mem::size_of::<u32>(),
        );
    }

    Ok(names_count as i64)
}

// ── Energy / GPU data ─────────────────────────────────────────────────────────

pub fn collect_energy(pid: i32) -> anyhow::Result<EnergyData> {
    let mut info: ffi::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let ret = unsafe {
        ffi::proc_pid_rusage(
            pid,
            2, // RUSAGE_INFO_V2
            &mut info as *mut _ as *mut _,
        )
    };
    anyhow::ensure!(ret == 0, "proc_pid_rusage failed for pid {pid}: {ret}");

    Ok(EnergyData {
        cpu_energy_nj: info.ri_billed_energy as i64,
        // GPU time is not directly available; ri_cpu_time_qos_user_interactive
        // is a proxy for UI/compositor activity (nanoseconds → milliseconds)
        gpu_time_ms: (info.ri_cpu_time_qos_user_interactive / 1_000_000) as i64,
    })
}
