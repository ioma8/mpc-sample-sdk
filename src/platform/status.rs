//! The app's status line.
//!
//! Every sample action reports through one fixed buffer that the paint hook
//! draws and the acceptance tests read out of `/proc/<pid>/mem` by symbol.  The
//! buffer therefore stays a plain static with its name intact; only the
//! formatting moved.

use core::sync::atomic::{AtomicU8, Ordering};

/// Longest status line, including the terminating NUL.
pub const STATUS_BYTES: usize = 96;
/// Longest action name plus ": " plus the message.
const MESSAGE_BYTES: usize = STATUS_BYTES - 1;

/// The status the menu shows.  A byte array rather than a `String` so the
/// symbol the tests read is the storage itself; the export name is the one the
/// glue and the acceptance tests already use.
#[unsafe(export_name = "g_hpss_status")]
pub static mut STATUS: [u8; STATUS_BYTES] = [0; STATUS_BYTES];

/// Serialises writers so two threads cannot interleave half a message.
static LOCK: AtomicU8 = AtomicU8::new(0);

/// The longest prefix of `text` that fits in `room` bytes without splitting a
/// character.
fn cut(text: &str, room: usize) -> &str {
    let mut end = room.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The status line, as an owned string.
#[must_use]
pub fn text() -> String {
    let mut bytes = [0_u8; STATUS_BYTES];
    lock();
    // SAFETY: `LOCK` is held, so no writer can change the buffer.
    unsafe {
        core::ptr::copy_nonoverlapping(
            core::ptr::addr_of!(STATUS).cast::<u8>(),
            bytes.as_mut_ptr(),
            STATUS_BYTES,
        );
    }
    unlock();
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(0);
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Replace the status line.
pub fn set_status(text: &str) {
    let text = cut(text, MESSAGE_BYTES);
    let mut bytes = [0_u8; STATUS_BYTES];
    bytes[..text.len()].copy_from_slice(text.as_bytes());
    write(&bytes);
}

/// Replace the status line with `"<prefix>: <message>"`.
pub fn set_prefixed(prefix: &str, message: &str) {
    let prefix = cut(prefix, MESSAGE_BYTES - 2);
    let room = MESSAGE_BYTES.saturating_sub(prefix.len() + 2);
    let message = cut(message, room);
    let mut text = [0_u8; STATUS_BYTES];
    text[..prefix.len()].copy_from_slice(prefix.as_bytes());
    text[prefix.len()..prefix.len() + 2].copy_from_slice(b": ");
    let at = prefix.len() + 2;
    text[at..at + message.len()].copy_from_slice(message.as_bytes());
    write(&text);
}

/// Replace the buffer through a raw pointer, so no `&mut` to a static is ever
/// formed.
fn write(bytes: &[u8; STATUS_BYTES]) {
    lock();
    // SAFETY: `LOCK` is held, so no other writer touches the buffer, and the
    // copy is exactly its size.
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            core::ptr::addr_of_mut!(STATUS).cast::<u8>(),
            STATUS_BYTES,
        );
    }
    unlock();
}

fn lock() {
    while LOCK
        .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

fn unlock() {
    LOCK.store(0, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::{STATUS_BYTES, set_prefixed, set_status};

    /// The status buffer is process-wide, so the tests that write it serialise;
    /// `cargo test` runs them in parallel otherwise.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn status() -> String {
        let mut bytes = [0_u8; STATUS_BYTES];
        // SAFETY: the tests serialise on this buffer, and the copy avoids
        // forming a reference to it.
        unsafe {
            core::ptr::copy_nonoverlapping(
                core::ptr::addr_of!(super::STATUS).cast::<u8>(),
                bytes.as_mut_ptr(),
                STATUS_BYTES,
            );
        }
        let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(0);
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn long_prefix_preserves_separator_and_terminator() {
        let _serial = serial();
        for prefix in ["x".repeat(94), "x".repeat(95), "€".repeat(40)] {
            set_prefixed(&prefix, "message");
            let value = super::text();
            assert!(value.len() < STATUS_BYTES);
            assert!(value.ends_with(": "));
        }
        // A prefix at the old limit must leave room for both separator bytes and
        // the NUL, not just two of the three.
        set_prefixed(&"x".repeat(95), "message");
        assert_eq!(super::text(), format!("{}: ", "x".repeat(93)));
    }

    #[test]
    fn a_status_is_prefixed_with_the_action() {
        let _serial = serial();
        set_prefixed("Bass Synth", "copy created");
        assert_eq!(status(), "Bass Synth: copy created");
        set_prefixed("HPSS", "no sample on this pad");
        assert_eq!(status(), "HPSS: no sample on this pad");
    }

    #[test]
    fn a_long_message_is_truncated_and_still_terminated() {
        let _serial = serial();
        let long = "x".repeat(STATUS_BYTES * 2);
        set_prefixed("Reverb", &long);
        let text = status();
        assert!(text.len() < STATUS_BYTES, "{}", text.len());
        assert!(text.starts_with("Reverb: xxx"));
        // Clearing works the same way.
        set_status("");
        assert_eq!(status(), "");
    }

    #[test]
    fn a_multibyte_message_is_cut_on_a_character_boundary() {
        let _serial = serial();
        // A three-byte character repeated past both limits: cutting by byte at
        // the wrong place would leave invalid UTF-8 in the line.
        let message = "€".repeat(40);
        set_prefixed("Bass Synth", &message);
        let text = status();
        assert!(text.starts_with("Bass Synth: "));
        assert!(text.ends_with('€'), "{text:?}");
        assert!(text.len() < STATUS_BYTES);
        set_status(&message);
        let text = status();
        assert!(text.ends_with('€'), "{text:?}");
        assert!(text.len() < STATUS_BYTES);
    }
}
