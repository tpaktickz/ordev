// PAL — global key state poll.
//
// Windows: `GetAsyncKeyState`, usable for any virtual-key code.
// Linux:   there is no global key *state* API; the answer comes from the evdev
//          hook (see the port plan P2). Until that backend lands the poll is
//          answered with `None` so callers fall back to event-driven input.

#[cfg(windows)]
pub fn key_down(vk: i32) -> Option<bool> {
    #[link(name = "user32")]
    extern "system" {
        fn GetAsyncKeyState(v_key: i32) -> i16;
    }
    Some(unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 })
}

#[cfg(not(windows))]
pub fn key_down(_vk: i32) -> Option<bool> {
    None
}
