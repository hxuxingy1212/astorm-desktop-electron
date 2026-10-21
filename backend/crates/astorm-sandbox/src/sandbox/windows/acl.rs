//! 原生 DACL 授权/撤销，替代逐路径 `icacls`。
//!
//! `icacls` 每条路径都要 fork 一个进程；在 `~/.cargo`（registry）、`~/.rustup`
//! （toolchains）、`AppData` 这类大目录上实测单条 10–30 秒，一条沙箱命令会被
//! 拖慢近分钟。这里直接调用 Win32 安全 API：
//!
//! `GetNamedSecurityInfoW` → `SetEntriesInAclW` → `SetNamedSecurityInfoW`
//!
//! 修改目录 DACL 是 O(1) 操作，耗时毫秒级。失败时调用方回退 `icacls`。

use std::ffi::c_void;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertStringSidToSidW, GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW,
    ACCESS_MODE, DENY_ACCESS, EXPLICIT_ACCESS_W, GRANT_ACCESS, REVOKE_ACCESS, SE_FILE_OBJECT,
    TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows::Win32::Security::{
    ACL, ACE_FLAGS, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, NO_INHERITANCE,
    OBJECT_INHERIT_ACE, PSECURITY_DESCRIPTOR, PSID,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 路径转宽字符：`SetNamedSecurityInfoW` 不走 Rust std 的长路径处理，
/// 超过 MAX_PATH 的目录需要 `\\?\` 扩展长度形式。
fn wide_path(path: &str) -> Vec<u16> {
    let lp = super::ffi::long_path(std::path::Path::new(path));
    wide(&lp.to_string_lossy())
}

fn sid_of(sid: &str) -> Option<PSID> {
    let w = wide(sid);
    let mut psid = PSID(std::ptr::null_mut());
    // SAFETY: w 以 NUL 结尾；psid 由 ConvertStringSidToSidW 分配，调用方负责 LocalFree。
    let ok = unsafe { ConvertStringSidToSidW(PCWSTR(w.as_ptr()), &mut psid) };
    ok.ok().map(|_| psid)
}

fn free(ptr: *mut c_void) {
    if !ptr.is_null() {
        // SAFETY: ptr 来自 Win32 的 LocalAlloc（GetNamedSecurityInfo / SetEntriesInAcl）。
        unsafe {
            let _ = LocalFree(Some(HLOCAL(ptr)));
        }
    }
}

fn explicit(psid: PSID, rights: u32, mode: ACCESS_MODE, inherit: ACE_FLAGS) -> EXPLICIT_ACCESS_W {
    let mut trustee = TRUSTEE_W::default();
    trustee.TrusteeForm = TRUSTEE_IS_SID;
    trustee.TrusteeType = TRUSTEE_IS_USER;
    // TrusteeForm = SID 时，ptstrName 指向 SID 本身（类型上借用为 PWSTR）。
    trustee.ptstrName = PWSTR(psid.0 as *mut u16);
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: rights,
        grfAccessMode: mode,
        grfInheritance: inherit,
        Trustee: trustee,
    }
}

/// 授予 `sid` 对 `path` 的访问权限。`inherit` 且目标为目录时向下继承。
pub fn grant(path: &str, sid: &str, rights: u32, inherit: bool) -> bool {
    apply(path, sid, rights, GRANT_ACCESS, inherit)
}

/// 在 `path` 上为 `sid` 添加拒绝 ACE。
pub fn deny(path: &str, sid: &str, rights: u32, inherit: bool) -> bool {
    apply(path, sid, rights, DENY_ACCESS, inherit)
}

/// 撤销 `path` 上属于 `sid` 的全部 ACE（会话结束时清理）。
pub fn revoke(path: &str, sid: &str) -> bool {
    let Some(psid) = sid_of(sid) else {
        return false;
    };
    // SAFETY: psid 在本调用期间有效；EXPLICIT_ACCESS_W 仅借用它。
    let ok = unsafe {
        change_dacl(
            path,
            &[explicit(psid, 0, REVOKE_ACCESS, NO_INHERITANCE)],
        )
    };
    free(psid.0);
    ok
}

fn apply(path: &str, sid: &str, rights: u32, mode: ACCESS_MODE, inherit: bool) -> bool {
    let Some(psid) = sid_of(sid) else {
        return false;
    };
    let is_dir = std::fs::metadata(path)
        .map(|m| m.is_dir())
        .unwrap_or(false);
    let flags = if inherit && is_dir {
        ACE_FLAGS(CONTAINER_INHERIT_ACE.0 | OBJECT_INHERIT_ACE.0)
    } else {
        NO_INHERITANCE
    };
    // SAFETY: psid 在本调用期间有效；EXPLICIT_ACCESS_W 仅借用它。
    let ok = unsafe { change_dacl(path, &[explicit(psid, rights, mode, flags)]) };
    free(psid.0);
    ok
}

unsafe fn change_dacl(path: &str, entries: &[EXPLICIT_ACCESS_W]) -> bool {
    let w = wide_path(path);
    let mut sd = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
    let mut old_dacl: *mut ACL = std::ptr::null_mut();
    // SAFETY: 输出指针均指向本栈帧有效变量；w 以 NUL 结尾。
    let err = unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(w.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut old_dacl),
            None,
            &mut sd,
        )
    };
    if err != ERROR_SUCCESS {
        return false;
    }
    // NULL DACL 表示“所有人完全访问”；换成只含新 ACE 的 DACL 会破坏现有权限，
    // 这种情况交给 icacls 回退处理。
    if old_dacl.is_null() {
        free(sd.0);
        return false;
    }
    let mut new_dacl: *mut ACL = std::ptr::null_mut();
    // SAFETY: entries 与 old_dacl 在调用期间有效；new_dacl 由 API 分配。
    let err = unsafe { SetEntriesInAclW(Some(entries), Some(old_dacl as *const ACL), &mut new_dacl) };
    if err != ERROR_SUCCESS {
        free(sd.0);
        return false;
    }
    // SAFETY: w 以 NUL 结尾；new_dacl 是刚构造的有效 DACL。
    let err = unsafe {
        SetNamedSecurityInfoW(
            PCWSTR(w.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(new_dacl as *const ACL),
            None,
        )
    };
    free(new_dacl as *mut c_void);
    free(sd.0);
    err == ERROR_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 原生授权/撤销在自建临时目录上可用（不依赖管理员权限）。
    #[test]
    fn native_grant_and_revoke_temp_dir() {
        let dir = std::env::temp_dir().join(format!(
            "astorm-acl-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().to_string();
        // S-1-5-32-544 = BUILTIN\Administrators（仅测试 ACE 读写，不影响当前用户）
        assert!(grant(&path, "S-1-5-32-544", 0x1200A9, false), "native grant");
        assert!(revoke(&path, "S-1-5-32-544"), "native revoke");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 超过 MAX_PATH 的目录也能通过 `\\?\` 授权/撤销。
    #[test]
    fn native_grant_long_path() {
        let base = std::env::temp_dir().join("astorm-acl-long");
        let mut dir = base.clone();
        for _ in 0..8 {
            dir.push("0123456789abcdef0123456789abcdef");
        }
        if std::fs::create_dir_all(&dir).is_err() {
            // 系统未启用长路径支持：跳过（不是本测试的失败）
            return;
        }
        let path = dir.to_string_lossy().to_string();
        if path.len() > 260 {
            assert!(grant(&path, "S-1-5-32-544", 0x1200A9, false), "long path grant");
            assert!(revoke(&path, "S-1-5-32-544"), "long path revoke");
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
