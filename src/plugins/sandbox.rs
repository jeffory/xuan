//! Blocking the network for plugins that declare no network hosts.
//!
//! With "Block network for plugins that don't declare it" on, Xuan starts a
//! plugin whose manifest lists no `permissions.network` hosts so that the
//! operating system keeps it off the network: under a seccomp filter on
//! Linux, in an AppContainer on Windows. When that cannot be done, the
//! plugin does not start. It is never run unblocked while the setting is on.
//!
//! # Linux
//!
//! The filter is installed in the child between `fork` and `exec`, so it
//! covers the plugin's command, its interpreter and every process it starts,
//! and it cannot be removed. It makes these system calls fail with `EACCES`:
//!
//! - `socket` and `socketpair` for every address family except `AF_UNIX`
//!   and `AF_NETLINK`. An allow-list rather than a deny-list, so that
//!   families which also reach other machines (`AF_PACKET`, `AF_SMC`,
//!   `AF_VSOCK`, `AF_BLUETOOTH`, …) are covered too. `AF_UNIX` keeps local
//!   IPC working. `AF_NETLINK` only talks to the local kernel, never to
//!   another machine, and C libraries use it to list interfaces (glibc's
//!   `getaddrinfo` and `if_nameindex`), so blocking it would break programs
//!   for no gain. `localhost` is blocked like any other address.
//! - `io_uring_setup`, because an io_uring can open sockets
//!   (`IORING_OP_SOCKET`) without calling `socket`.
//!
//! Only 64-bit x86_64 and aarch64 are supported (riscv64 too, untested). The
//! filter starts with an architecture check that kills the process for any
//! other syscall ABI, so a 64-bit plugin cannot reach `socketcall` through
//! the i386 `int 0x80` or arm32 compat entry points. The x32 ABI reports
//! `AUDIT_ARCH_X86_64` with the `__X32_SYSCALL_BIT` set in the syscall
//! number, so the x32 numbers are listed explicitly. A 32-bit plugin
//! program does not run at all under the filter.
//!
//! The filter is not a sandbox: the plugin still runs with the user's
//! rights, and anything it can reach over a Unix socket (D-Bus, a systemd
//! user manager, a local proxy) can connect for it.
//!
//! # Windows
//!
//! The plugin runs in an AppContainer named `Xuan.Plugin.<id>` (see
//! [`container_name`]), created on first use and kept afterwards, with no
//! capabilities: neither `internetClient` nor `privateNetworkClientServer`.
//! Windows refuses its connections to other machines and to `localhost`:
//! an AppContainer never reaches loopback without a firewall exemption,
//! which needs administrator rights. Its subprocesses inherit the container.
//! See `windows.rs` for how it is started.
//!
//! The container confines files too: it opens only what its SID, or `ALL
//! APPLICATION PACKAGES`, is allowed in the access control lists. So before
//! the plugin starts, Xuan adds inheritable allow entries for the
//! container's SID to these folders, unless one is already there:
//!
//! - read and execute: the plugin folder, the folder of the program it runs
//!   when that is outside the plugin folder (for `python3`, the folder of the
//!   `python3.exe` found on `PATH`, which also holds the standard library),
//!   and a virtual environment's base interpreter (`home` in `pyvenv.cfg`).
//!   Folders `ALL APPLICATION PACKAGES` can already read, such as
//!   `C:\Windows` and `C:\Program Files`, are left alone; changing them
//!   would need administrator rights. When the interpreter's folder cannot
//!   be changed the plugin still starts, with a note in its log, and fails
//!   there if the interpreter cannot run.
//! - read, write and delete ([`share`]): its data folder, its scratch
//!   folder, which is also its `TEMP` and `TMP` ([`temp_env`]), and the
//!   `work_dir` of each job, import and export. A file-format plugin cannot
//!   open the user's file itself, so the editor copies it into the import's
//!   `work_dir`, and an export's file out of it.
//!
//! The entries stay, so a later start finds them and changes nothing, and so
//! does the container profile.
use std::path::Path;

use super::manifest::Permissions;

/// Whether Xuan can block a plugin's network on this platform.
pub const SUPPORTED: bool = cfg!(any(target_os = "linux", windows));

/// Whether a blocked plugin can open only the files Xuan shares with it
/// ([`share`]), and so needs its own temporary folder ([`temp_env`]).
pub const CONFINES_FILES: bool = cfg!(windows);

/// What the permission dialog and Manage Plugins say for a blocked plugin.
pub const BLOCKED_LABEL: &str = if cfg!(windows) {
    "Network blocked by Xuan (Windows)"
} else {
    "Network blocked by Xuan (Linux)"
};

/// The first line of a blocked plugin's log.
pub const LOG_NOTE: &str = if cfg!(windows) {
    "Network blocked by Xuan: the plugin runs in an AppContainer without network capabilities, so its connections fail, also to localhost"
} else {
    "Network blocked by Xuan: opening sockets other than Unix sockets fails with EACCES"
};

/// Whether a plugin runs with its network blocked: the setting is on, the
/// platform supports it, and the plugin declares no network hosts. A plugin
/// that declares hosts is never blocked; the send prompt is its control.
pub fn blocks_network(setting: bool, permissions: &Permissions) -> bool {
    SUPPORTED && setting && permissions.network.is_empty()
}

#[cfg(target_os = "linux")]
pub use linux::{available, block_network};

/// Arrange for `command` to start with its network blocked.
#[cfg(not(any(target_os = "linux", windows)))]
pub fn block_network(_command: &mut std::process::Command) -> anyhow::Result<()> {
    anyhow::bail!("Xuan can block a plugin's network only on Linux and Windows")
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{Contained, spawn};

#[cfg(any(windows, test))]
mod plan;

/// Let the blocked plugin `plugin` read, write and delete in `dir` and in
/// everything created there later. Needed only where [`CONFINES_FILES`];
/// elsewhere it does nothing.
pub fn share(plugin: &str, dir: &Path) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        windows::share(plugin, dir)
    }
    #[cfg(not(windows))]
    {
        let _ = (plugin, dir);
        Ok(())
    }
}

/// The environment that makes `scratch` a blocked plugin's temporary
/// folder where [`CONFINES_FILES`], since the user's own is closed to it.
/// Empty elsewhere.
pub fn temp_env(scratch: &Path) -> Vec<(String, String)> {
    if !CONFINES_FILES {
        return Vec::new();
    }
    let scratch = scratch.display().to_string();
    vec![("TEMP".into(), scratch.clone()), ("TMP".into(), scratch)]
}

/// The AppContainer a plugin runs in on Windows: `Xuan.Plugin.<id>`. A name
/// has at most 64 characters, so a longer one keeps the start of the id
/// followed by a hash of all of it.
#[cfg(any(windows, test))]
pub fn container_name(plugin: &str) -> String {
    const PREFIX: &str = "Xuan.Plugin.";
    const MAX: usize = 64;
    let name = format!("{PREFIX}{plugin}");
    if name.len() <= MAX {
        return name;
    }
    // FNV-1a: stable across releases, unlike std's hasher.
    let hash = (plugin.bytes()).fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    // Ids are ASCII, so every index is a character boundary.
    let kept = &plugin[..MAX - PREFIX.len() - 9];
    format!("{PREFIX}{kept}.{hash:08x}")
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{collections::BTreeMap, io, os::unix::process::CommandExt, process::Command};

    use anyhow::{Context, Result};
    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
        SeccompRule, TargetArch,
    };

    /// The errno a blocked call fails with.
    const BLOCKED_ERRNO: i32 = libc::EACCES;

    /// Set in the syscall number by the x32 ABI on x86_64.
    #[cfg(target_arch = "x86_64")]
    const X32_SYSCALL_BIT: i64 = 0x4000_0000;

    /// Whether this kernel, and the container Xuan may run in, let a process
    /// install the filter: seccomp filters exist and support the two
    /// actions it uses. Needs Linux 4.14 or later.
    pub fn available() -> Result<()> {
        for (action, name) in [
            (libc::SECCOMP_RET_ERRNO, "SECCOMP_RET_ERRNO"),
            (libc::SECCOMP_RET_KILL_PROCESS, "SECCOMP_RET_KILL_PROCESS"),
        ] {
            // SAFETY: SECCOMP_GET_ACTION_AVAIL reads one u32 through the
            // pointer, which points to a local that outlives the call. It
            // changes nothing.
            let result = unsafe {
                libc::syscall(
                    libc::SYS_seccomp,
                    libc::SECCOMP_GET_ACTION_AVAIL,
                    0,
                    &raw const action,
                )
            };
            if result != 0 {
                return Err(io::Error::last_os_error()).with_context(|| {
                    format!("this system does not allow seccomp filters with {name}")
                });
            }
        }
        Ok(())
    }

    /// The filter, compiled for the architecture Xuan was built for.
    fn filter() -> Result<BpfProgram> {
        let arch = TargetArch::try_from(std::env::consts::ARCH)
            .context("Xuan cannot block the network on this architecture")?;
        let other_family = || -> Result<Vec<SeccompRule>> {
            // The domain is an `int`, so only its low 32 bits count, as in
            // the kernel. Both conditions hold: neither AF_UNIX nor AF_NETLINK.
            let not = |family: i32| {
                SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Ne, family as u64)
            };
            Ok(vec![SeccompRule::new(vec![
                not(libc::AF_UNIX)?,
                not(libc::AF_NETLINK)?,
            ])?])
        };
        let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
        for syscall in [libc::SYS_socket, libc::SYS_socketpair] {
            rules.insert(syscall, other_family()?);
            #[cfg(target_arch = "x86_64")]
            rules.insert(syscall | X32_SYSCALL_BIT, other_family()?);
        }
        // No conditions: every call matches.
        rules.insert(libc::SYS_io_uring_setup, Vec::new());
        #[cfg(target_arch = "x86_64")]
        rules.insert(libc::SYS_io_uring_setup | X32_SYSCALL_BIT, Vec::new());
        let filter = SeccompFilter::new(
            rules,
            SeccompAction::Allow,
            SeccompAction::Errno(BLOCKED_ERRNO as u32),
            arch,
        )?;
        Ok(BpfProgram::try_from(filter)?)
    }

    /// Arrange for `command` to start with its network blocked. Fails when
    /// the filter cannot be used here; spawning fails when it cannot be
    /// installed in the child.
    pub fn block_network(command: &mut Command) -> Result<()> {
        available()?;
        // Compiled here, before `fork`: building it allocates.
        let program = filter()?;
        // SAFETY: `pre_exec` runs the closure in the forked child, where
        // only async-signal-safe work is allowed (another thread of Xuan may
        // have held the allocator's lock at the fork). The closure only
        // reads the program it owns and makes two system calls:
        // `seccompiler::apply_filter` (audited for the pinned 0.5.0) checks
        // that the slice is not empty, calls `prctl(PR_SET_NO_NEW_PRIVS)`
        // and `seccomp(SECCOMP_SET_MODE_FILTER)` with a `sock_fprog` on the
        // stack, and builds its error from `errno` without allocating. Its
        // error is turned into an `io::Error` holding an errno, which does
        // not allocate either; std passes that errno to the parent, where
        // `spawn` fails. Nothing is locked, allocated or freed.
        unsafe {
            command.pre_exec(move || install(&program));
        }
        Ok(())
    }

    /// Install the filter in the calling process. Runs between `fork` and
    /// `exec`; see the safety note in [`block_network`].
    fn install(program: &BpfProgram) -> io::Result<()> {
        seccompiler::apply_filter(program).map_err(|error| match error {
            seccompiler::Error::Prctl(error) | seccompiler::Error::Seccomp(error) => error,
            _ => io::Error::from_raw_os_error(libc::EINVAL),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // Offsets in `struct seccomp_data`, and BPF opcodes.
        const ARCH_OFFSET: u32 = 4;
        const LD_W_ABS: u16 = 0x20;
        const JEQ_K: u16 = 0x15;
        const RET_K: u16 = 0x06;

        #[test]
        fn the_filter_starts_by_killing_other_syscall_abis() {
            let program = filter().unwrap();
            let audit_arch = if cfg!(target_arch = "x86_64") {
                62 | 0x8000_0000 | 0x4000_0000 // AUDIT_ARCH_X86_64
            } else if cfg!(target_arch = "aarch64") {
                183 | 0x8000_0000 | 0x4000_0000 // AUDIT_ARCH_AARCH64
            } else {
                return;
            };
            let [load, check, kill, ..] = program.as_slice() else {
                panic!("filter too short: {program:?}");
            };
            assert_eq!((load.code, load.k), (LD_W_ABS, ARCH_OFFSET));
            assert_eq!(
                (check.code, check.k, check.jt, check.jf),
                (JEQ_K, audit_arch, 1, 0)
            );
            assert_eq!((kill.code, kill.k), (RET_K, libc::SECCOMP_RET_KILL_PROCESS));
            // Every other return either allows or fails with EACCES.
            let blocked = libc::SECCOMP_RET_ERRNO | BLOCKED_ERRNO as u32;
            for instruction in &program[3..] {
                if instruction.code == RET_K {
                    assert!(
                        [libc::SECCOMP_RET_ALLOW, blocked].contains(&instruction.k),
                        "{instruction:?}"
                    );
                }
            }
            // The syscall numbers it compares against, x32 ones included.
            let numbers: Vec<u32> = (program.iter())
                .filter(|i| i.code == JEQ_K)
                .map(|i| i.k)
                .collect();
            let mut expected = vec![
                libc::SYS_socket,
                libc::SYS_socketpair,
                libc::SYS_io_uring_setup,
            ];
            #[cfg(target_arch = "x86_64")]
            expected.extend([
                libc::SYS_socket | X32_SYSCALL_BIT,
                libc::SYS_socketpair | X32_SYSCALL_BIT,
                libc::SYS_io_uring_setup | X32_SYSCALL_BIT,
            ]);
            for number in expected {
                assert!(numbers.contains(&(number as u32)), "{number}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plugins_without_hosts_are_blocked_and_only_when_asked() {
        let none = Permissions::default();
        let hosts = Permissions {
            network: vec!["localhost".into()],
            ..Permissions::default()
        };
        // Other permissions do not matter.
        let editing = Permissions {
            document: super::super::manifest::DocumentAccess::Edit,
            secrets: vec!["key".into()],
            ..Permissions::default()
        };
        let platform = cfg!(any(target_os = "linux", windows));
        assert_eq!(blocks_network(true, &none), platform);
        assert_eq!(blocks_network(true, &editing), platform);
        assert!(!blocks_network(true, &hosts));
        assert!(!blocks_network(false, &none));
        assert!(!blocks_network(false, &hosts));
    }

    #[test]
    fn container_names_fit_in_64_characters_and_stay_distinct() {
        assert_eq!(container_name("histogram"), "Xuan.Plugin.histogram");
        let long = "a".repeat(52);
        assert_eq!(container_name(&long), format!("Xuan.Plugin.{long}"));
        let longer = "a".repeat(64);
        let other = format!("{}b", "a".repeat(63));
        let (name, other_name) = (container_name(&longer), container_name(&other));
        assert_eq!((name.len(), other_name.len()), (64, 64));
        assert_ne!(name, other_name);
        assert!(name.starts_with(&format!("Xuan.Plugin.{}.", "a".repeat(43))));
        // Stable across releases: a new name would be a new container,
        // without the access granted to the old one.
        assert_eq!(name, format!("Xuan.Plugin.{}.d96f0f85", "a".repeat(43)));
        // The characters Windows allows: [-_. A-Za-z0-9].
        let allowed = |b: u8| b.is_ascii_alphanumeric() || b"._-".contains(&b);
        assert!(name.bytes().all(allowed));
    }

    #[test]
    fn a_private_temporary_folder_only_where_files_are_confined() {
        let env = temp_env(Path::new("scratch"));
        assert_eq!(env.is_empty(), !CONFINES_FILES);
        if !CONFINES_FILES {
            share("p", Path::new("missing")).unwrap();
        }
        let label = if cfg!(windows) {
            "(Windows)"
        } else {
            "(Linux)"
        };
        assert!(BLOCKED_LABEL.ends_with(label));
        assert!(LOG_NOTE.starts_with("Network blocked by Xuan"));
    }
}
