// OpenRevo - Linux desktop-wide shortcuts through the XDG portal
// Wayland does not let an application grab a key for itself, so the only sanctioned
// route is org.freedesktop.portal.GlobalShortcuts: the compositor owns the grab and
// tells us when it fires. The portal requires an app id, which it derives from the
// caller's cgroup / desktop entry, so this path is only live for packaged runs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::AppHandle;
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::MatchRule;

use super::dispatcher::dispatch_action;
use super::types::{ParsedKeyCombo, ShortcutBinding};

const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE_SHORTCUTS: &str = "org.freedesktop.portal.GlobalShortcuts";
const IFACE_REQUEST: &str = "org.freedesktop.portal.Request";

/// True once BindShortcuts succeeded: the compositor is grabbing the keys, so the
/// evdev matcher must stay out of the way or every press would fire twice.
static PORTAL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Every shortcut id the portal currently knows about, kept in sync with the registry.
static BOUND: Mutex<Vec<ShortcutBinding>> = Mutex::new(Vec::new());

/// Live portal session. Kept so a settings change can re-bind without a restart.
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// Host handle, kept so a settings change can re-bind without threading it through.
static APP: Mutex<Option<AppHandle>> = Mutex::new(None);

struct Session {
    conn: Connection,
    handle: OwnedObjectPath,
}

/// True when the compositor is the one grabbing the keys.
pub fn active() -> bool {
    PORTAL_ACTIVE.load(Ordering::SeqCst)
}

/// Bind every enabled shortcut at startup. Safe to call more than once.
pub fn start(app: Option<AppHandle>) {
    *APP.lock().unwrap() = app.clone();
    let bindings = enabled_bindings();
    if bindings.is_empty() {
        crate::log_info!(
            "SHORTCUTS",
            "No custom shortcuts configured; the portal bridge stays idle"
        );
        return;
    }
    std::thread::spawn(move || match connect_and_bind(&bindings) {
        Ok((session, granted)) => {
            *SESSION.lock().unwrap() = Some(session);
            *BOUND.lock().unwrap() = bindings.clone();
            // Only claim the keys once the compositor has them, otherwise a portal
            // that answered with zero shortcuts would silently kill the evdev path.
            if granted > 0 {
                PORTAL_ACTIVE.store(true, Ordering::SeqCst);
                crate::log_info!(
                    "SHORTCUTS",
                    "Desktop portal holds {} global shortcut(s); key grabbing is compositor-owned",
                    granted
                );
            }
            listen(app);
        }
        Err(reason) => {
            crate::log_warn!(
                "SHORTCUTS",
                "Desktop portal shortcuts unavailable ({}); staying on the evdev matcher",
                reason
            );
        }
    });
}

/// Re-bind after the shortcut list changed. Falls back to a fresh start when the
/// session never came up.
pub fn rebind(bindings: Vec<ShortcutBinding>) {
    let app = APP.lock().unwrap().clone();
    let mut guard = SESSION.lock().unwrap();
    let Some(session) = guard.as_mut() else {
        drop(guard);
        // No live session: (re)start, even without a Tauri handle the daemon can
        // still drive hardware, it just cannot broadcast to webviews.
        start(app);
        return;
    };
    if bindings.is_empty() {
        *BOUND.lock().unwrap() = Vec::new();
        PORTAL_ACTIVE.store(false, Ordering::SeqCst);
        return;
    }
    match bind_shortcuts(&session.conn, &session.handle, &bindings) {
        Ok(granted) => {
            *BOUND.lock().unwrap() = bindings;
            PORTAL_ACTIVE.store(granted > 0, Ordering::SeqCst);
            crate::log_info!("SHORTCUTS", "Re-bound {} portal shortcut(s)", granted);
        }
        Err(reason) => {
            crate::log_warn!("SHORTCUTS", "Re-binding portal shortcuts failed: {}", reason)
        }
    }
}

fn enabled_bindings() -> Vec<ShortcutBinding> {
    super::manager::get_all_shortcuts()
        .into_iter()
        .filter(|b| b.enabled)
        .collect()
}

fn connect_and_bind(bindings: &[ShortcutBinding]) -> Result<(Session, usize), String> {
    let conn = Connection::session().map_err(|e| format!("no session bus: {e}"))?;
    let handle = create_session(&conn)?;
    let granted = bind_shortcuts(&conn, &handle, bindings)?;
    Ok((Session { conn, handle }, granted))
}

fn create_session(conn: &Connection) -> Result<OwnedObjectPath, String> {
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("handle_token", Value::from("openrevo1"));
    options.insert("session_handle_token", Value::from("openrevo1"));
    let results = request(
        conn,
        IFACE_SHORTCUTS,
        "CreateSession",
        "openrevo1",
        &(&options,),
    )?;
    let raw = results
        .get("session_handle")
        .ok_or_else(|| "portal returned no session handle".to_string())?;
    // xdg-desktop-portal versions disagree here: the GlobalShortcuts backend answers
    // with an object path, the generic request plumbing can hand back the same value
    // as a string. Accept both instead of dying on the signature.
    if let Ok(path) = raw.downcast_ref::<zbus::zvariant::ObjectPath<'_>>() {
        return Ok(OwnedObjectPath::from(path.clone()));
    }
    if let Ok(text) = String::try_from(raw.clone()) {
        return OwnedObjectPath::try_from(text).map_err(|e| format!("bad session handle: {e}"));
    }
    Err(format!(
        "bad session handle: unexpected type {}",
        raw.value_signature()
    ))
}

fn bind_shortcuts(
    conn: &Connection,
    session: &OwnedObjectPath,
    bindings: &[ShortcutBinding],
) -> Result<usize, String> {
    let mut shortcuts: Vec<(String, HashMap<String, Value>)> = Vec::new();
    for binding in bindings {
        let mut entry: HashMap<String, Value> = HashMap::new();
        entry.insert("description".to_string(), Value::from(binding.name.clone()));
        if let Some(trigger) = trigger_for(&binding.combo) {
            entry.insert("preferred_trigger".to_string(), Value::from(trigger));
        }
        shortcuts.push((binding.id.clone(), entry));
    }
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("handle_token", Value::from("openrevo2"));
    let results = request(
        conn,
        IFACE_SHORTCUTS,
        "BindShortcuts",
        "openrevo2",
        &(session, &shortcuts, &"", &options),
    )?;
    // The answer carries the accepted list; an empty answer means the user declined.
    let granted = results
        .get("shortcuts")
        .and_then(|value| Vec::<OwnedValue>::try_from(value.clone()).ok())
        .map(|list| list.len())
        .unwrap_or(0);
    Ok(granted)
}

/// Fire the request, then wait for its Response signal.
///
/// The portal emits Response as a unicast signal on the caller's connection, so the
/// match rule has to be registered *before* the call and on the predicted request
/// path — the path is built from the sender's unique name plus our handle token.
fn request<B>(
    conn: &Connection,
    iface: &str,
    method: &str,
    token: &str,
    body: &B,
) -> Result<HashMap<String, OwnedValue>, String>
where
    B: serde::Serialize + zbus::zvariant::Type,
{
    let unique = conn
        .unique_name()
        .ok_or_else(|| "connection has no unique name".to_string())?
        .to_string();
    let sender = unique.trim_start_matches(':').replace('.', "_");
    let path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");

    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(IFACE_REQUEST)
        .map_err(|e| format!("bad interface name: {e}"))?
        .member("Response")
        .map_err(|e| format!("bad member name: {e}"))?
        .path(path.as_str())
        .map_err(|e| format!("bad request path: {e}"))?
        .build();
    let mut iterator = MessageIterator::for_match_rule(rule, conn, Some(8))
        .map_err(|e| format!("cannot watch the portal response: {e}"))?;

    conn.call_method(Some(PORTAL_DEST), PORTAL_PATH, Some(iface), method, body)
        .map_err(|e| format!("{method} refused: {e}"))?;

    let message = iterator
        .next()
        .ok_or_else(|| format!("{method} got no response"))?
        .map_err(|e| format!("{method} response unreadable: {e}"))?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = message
        .body()
        .deserialize()
        .map_err(|e| format!("{method} response malformed: {e}"))?;
    match code {
        0 => Ok(results),
        1 => Err(format!("{method} cancelled by the user")),
        other => Err(format!("{method} failed with portal response code {other}")),
    }
}

/// Sit on the Activated signal for the rest of the process lifetime.
fn listen(app: Option<AppHandle>) {
    let conn = match SESSION.lock().unwrap().as_ref() {
        Some(session) => session.conn.clone(),
        None => return,
    };
    let rule = match MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(IFACE_SHORTCUTS)
        .and_then(|b| b.member("Activated"))
        .and_then(|b| b.path(PORTAL_PATH))
    {
        Ok(builder) => builder.build(),
        Err(e) => {
            crate::log_warn!("SHORTCUTS", "Cannot watch portal activation: {}", e);
            return;
        }
    };
    let iterator = match MessageIterator::for_match_rule(rule, &conn, Some(32)) {
        Ok(iterator) => iterator,
        Err(e) => {
            crate::log_warn!("SHORTCUTS", "Cannot watch portal activation: {}", e);
            return;
        }
    };

    for message in iterator {
        let Ok(message) = message else { break };
        let Ok((_session, id, _timestamp, _options)): Result<
            (OwnedObjectPath, String, u64, HashMap<String, OwnedValue>),
            _,
        > = message.body().deserialize() else {
            continue;
        };
        let binding = BOUND
            .lock()
            .unwrap()
            .iter()
            .find(|b| b.id == id)
            .cloned();
        match binding {
            Some(binding) => {
                crate::log_info!("SHORTCUTS", "Portal shortcut fired: {} ({})", binding.name, id);
                dispatch_action(app.clone(), binding);
            }
            None => crate::log_warn!("SHORTCUTS", "Portal fired unknown shortcut id: {}", id),
        }
    }
    // Falling out of the loop means the bus connection dropped; the compositor no
    // longer holds the keys, so hand them back to the evdev matcher.
    PORTAL_ACTIVE.store(false, Ordering::SeqCst);
    crate::log_warn!("SHORTCUTS", "Portal activation stream ended; falling back to evdev");
}

/// Translate "Ctrl+Alt+M" into the XKB spelling the portal expects: "CTRL+ALT+m".
fn trigger_for(combo: &str) -> Option<String> {
    let parsed = ParsedKeyCombo::parse(combo)?;
    let key = keysym_for(parsed.vk_code)?;
    let mut trigger = String::new();
    for (down, name) in [
        (parsed.ctrl, "CTRL"),
        (parsed.alt, "ALT"),
        (parsed.shift, "SHIFT"),
        (parsed.win, "META"),
    ] {
        if down {
            trigger.push_str(name);
            trigger.push('+');
        }
    }
    trigger.push_str(&key);
    Some(trigger)
}

/// Virtual-key code to XKB keysym name, covering the keys the settings UI offers.
fn keysym_for(vk: u16) -> Option<String> {
    let plain = match vk {
        // Digits and letters: the keysym is the lower-case character itself.
        0x30..=0x39 => Some(((vk as u8) as char).to_string()),
        0x41..=0x5A => Some(((vk as u8).to_ascii_lowercase() as char).to_string()),
        // Function keys.
        0x70..=0x87 => Some(format!("F{}", vk - 0x70 + 1)),
        0x20 => Some("space".to_string()),
        0x09 => Some("Tab".to_string()),
        0x0D => Some("Return".to_string()),
        0x1B => Some("Escape".to_string()),
        0x08 => Some("BackSpace".to_string()),
        0x2E => Some("Delete".to_string()),
        0x2D => Some("Insert".to_string()),
        0x24 => Some("Home".to_string()),
        0x23 => Some("End".to_string()),
        0x21 => Some("Prior".to_string()),
        0x22 => Some("Next".to_string()),
        0x25 => Some("Left".to_string()),
        0x26 => Some("Up".to_string()),
        0x27 => Some("Right".to_string()),
        0x28 => Some("Down".to_string()),
        0x2C => Some("Print".to_string()),
        0x13 => Some("Pause".to_string()),
        0x14 => Some("Caps_Lock".to_string()),
        0x90 => Some("Num_Lock".to_string()),
        0x91 => Some("Scroll_Lock".to_string()),
        0xBD => Some("minus".to_string()),
        0xBB => Some("equal".to_string()),
        0xDB => Some("bracketleft".to_string()),
        0xDD => Some("bracketright".to_string()),
        0xBA => Some("semicolon".to_string()),
        0xDE => Some("apostrophe".to_string()),
        0xBC => Some("comma".to_string()),
        0xBE => Some("period".to_string()),
        0xBF => Some("slash".to_string()),
        0xDC => Some("backslash".to_string()),
        0xC0 => Some("grave".to_string()),
        _ => None,
    };
    plain
}

#[cfg(test)]
mod tests {
    use super::trigger_for;

    #[test]
    fn translates_ctrl_alt_letter() {
        assert_eq!(trigger_for("Ctrl+Alt+M").as_deref(), Some("CTRL+ALT+m"));
    }

    #[test]
    fn translates_win_and_function_keys() {
        assert_eq!(trigger_for("Win+Shift+F5").as_deref(), Some("SHIFT+META+F5"));
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(trigger_for("Ctrl+Hyper").is_none());
    }
}
