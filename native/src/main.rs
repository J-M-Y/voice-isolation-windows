#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod audio;
mod controller;
mod processor;
mod ui;

#[cfg(windows)]
mod instance {
    use std::{ffi::c_void, ptr};

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(attributes: *const c_void, initial_owner: i32, name: *const u16) -> *mut c_void;
        fn GetLastError() -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(window: *mut c_void, text: *const u16, title: *const u16, flags: u32) -> i32;
    }
    fn wide(value: &str) -> Vec<u16> { value.encode_utf16().chain(Some(0)).collect() }
    pub fn notify(message: &str) {
        unsafe { MessageBoxW(ptr::null_mut(), wide(message).as_ptr(), wide("指定人声提取").as_ptr(), 0x40); }
    }
    pub struct Guard(*mut c_void);
    impl Guard {
        pub fn acquire() -> Result<Option<Self>, String> {
            // Local namespace gives each Windows login session its own instance.
            // Share the existing Python edition's mutex so both versions
            // cannot capture the same devices simultaneously.
            let name = wide("Local\\VoiceIsolation_3E5B771A_Tray_v1.mutex");
            let handle = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
            if handle.is_null() { return Err(format!("无法创建单实例保护（Windows 错误 {}）", unsafe { GetLastError() })); }
            if unsafe { GetLastError() } == 183 {
                unsafe { CloseHandle(handle); }
                return Ok(None);
            }
            Ok(Some(Self(handle)))
        }
    }
    impl Drop for Guard { fn drop(&mut self) { unsafe { CloseHandle(self.0); } } }
}

fn main() {
    #[cfg(windows)]
    let _instance = match instance::Guard::acquire() {
        Ok(Some(guard)) => guard,
        Ok(None) => { instance::notify("程序已在运行。请从系统托盘打开窗口。"); return; },
        Err(error) => { instance::notify(&error); return; },
    };
    let (commands, events, worker) = controller::start();
    if let Err(error) = ui::run(commands, events) {
        #[cfg(windows)]
        instance::notify(&format!("无法启动界面：{error}"));
        eprintln!("无法启动界面: {error}");
    }
    // Audio drivers or model inference can hang while shutting down. Give
    // cleanup time to finish, but never leave an invisible process holding
    // the single-instance mutex indefinitely.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !worker.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if worker.is_finished() { let _ = worker.join(); }
}
