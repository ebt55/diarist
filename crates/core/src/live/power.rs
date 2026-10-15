//! Power while recording (`docs/01`, "Capture robustness"): keep the system
//! awake (`SetThreadExecutionState(ES_SYSTEM_REQUIRED | ES_CONTINUOUS)`,
//! which holds for the calling thread until it is cleared or the thread
//! ends) and hear about suspend and resume
//! (`PowerRegisterSuspendResumeNotification` with a callback), so the
//! recorder can flush and anchor before the system sleeps and reopen the
//! captures after it wakes.

/// A suspend/resume notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    Suspend,
    Resume,
}

/// Keeps the system awake while alive; must be dropped on the thread that
/// created it.
pub struct KeepAwake {
    _not_send: std::marker::PhantomData<*const ()>,
}

impl KeepAwake {
    pub fn new() -> Self {
        #[cfg(windows)]
        {
            use windows::Win32::System::Power::{
                SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
            };
            // SAFETY: plain flag call on the current thread.
            unsafe {
                SetThreadExecutionState(ES_SYSTEM_REQUIRED | ES_CONTINUOUS);
            }
        }
        Self {
            _not_send: std::marker::PhantomData,
        }
    }
}

impl Default for KeepAwake {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS};
            // SAFETY: as above; ES_CONTINUOUS alone clears the requirement.
            unsafe {
                SetThreadExecutionState(ES_CONTINUOUS);
            }
        }
    }
}

type Callback = Box<dyn Fn(PowerEvent) + Send + Sync>;

/// A registered suspend/resume callback; unregistered on drop.
pub struct PowerNotifications {
    #[cfg(windows)]
    handle: *mut core::ffi::c_void,
    #[cfg(windows)]
    params: *mut windows::Win32::System::Power::DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS,
    callback: *mut Callback,
}

// SAFETY: the raw pointers are only touched in `register` and `drop`; the
// callback itself is `Send + Sync`.
unsafe impl Send for PowerNotifications {}
unsafe impl Sync for PowerNotifications {}

#[cfg(windows)]
unsafe extern "system" fn on_power(
    context: *const core::ffi::c_void,
    kind: u32,
    _setting: *const core::ffi::c_void,
) -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{
        PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
    };
    if context.is_null() {
        return 0;
    }
    // SAFETY: `context` is the `Box<Callback>` leaked in `register`, alive
    // until the registration is dropped (which unregisters first).
    let cb = unsafe { &*(context as *const Callback) };
    match kind {
        PBT_APMSUSPEND => cb(PowerEvent::Suspend),
        PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => cb(PowerEvent::Resume),
        _ => {}
    }
    0
}

impl PowerNotifications {
    /// Registers `callback` for suspend and resume (`None` when the system
    /// refuses or on other platforms).
    pub fn register(callback: impl Fn(PowerEvent) + Send + Sync + 'static) -> Option<Self> {
        let callback: *mut Callback = Box::into_raw(Box::new(Box::new(callback)));
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::System::Power::{
                PowerRegisterSuspendResumeNotification, DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS,
            };
            use windows::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_CALLBACK;
            let params = Box::into_raw(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
                Callback: Some(on_power),
                Context: callback.cast(),
            }));
            let mut handle: *mut core::ffi::c_void = std::ptr::null_mut();
            // SAFETY: `params` and `callback` stay allocated until drop.
            let rc = unsafe {
                PowerRegisterSuspendResumeNotification(
                    DEVICE_NOTIFY_CALLBACK,
                    HANDLE(params.cast()),
                    &mut handle,
                )
            };
            if rc.0 != 0 {
                // SAFETY: not registered, so nothing else holds these.
                unsafe {
                    drop(Box::from_raw(params));
                    drop(Box::from_raw(callback));
                }
                return None;
            }
            Some(Self {
                handle,
                params,
                callback,
            })
        }
        #[cfg(not(windows))]
        {
            // SAFETY: never shared.
            unsafe { drop(Box::from_raw(callback)) };
            None
        }
    }
}

impl Drop for PowerNotifications {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use windows::Win32::System::Power::{
                PowerUnregisterSuspendResumeNotification, HPOWERNOTIFY,
            };
            // SAFETY: unregister first, then free what the callback used.
            unsafe {
                let _ =
                    PowerUnregisterSuspendResumeNotification(HPOWERNOTIFY(self.handle as isize));
                drop(Box::from_raw(self.params));
            }
        }
        // SAFETY: unregistered above, so the callback can no longer run.
        unsafe { drop(Box::from_raw(self.callback)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_awake_and_registration_round_trip() {
        let _awake = KeepAwake::new();
        let reg = PowerNotifications::register(|_| {});
        if cfg!(windows) {
            assert!(reg.is_some(), "suspend/resume registration failed");
        }
        drop(reg);
    }
}
