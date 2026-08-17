//! One-time join codes.
//!
//! The TV shows a QR encoding `/join?c=<code>`. Scanning it exchanges the code
//! once for a per-person token, and the code is burned.
//!
//! **Codes live in memory only.** They are secrets, and the event log is
//! append-only and served unauthenticated on the LAN — nothing that must ever
//! stop being true belongs in it.
//!
//! It is worth being precise about what rotation buys, because the obvious
//! framing does not survive the threat model. Anyone already on the LAN can
//! simply fetch the QR endpoint, and the LAN is SESH's declared trust boundary,
//! so rotation defends against none of them. What it does defeat is a
//! **photograph of the TV, used later**: the guest who has since left, the
//! picture in someone's camera roll, the frame in a streamed clip. Burning the
//! code on exchange and rotating it every minute bounds that exposure to the
//! minute it was on screen.

use std::sync::Mutex;

/// How long a code stays on the TV before it is replaced.
pub const ROTATE_MS: i64 = 60_000;

/// How long a just-replaced code keeps working.
///
/// Someone photographs the QR at 59.9s and their phone posts at 60.4s. Without
/// this the join fails for no reason a person could see or fix, and the fix
/// they would try — scan again — is the one thing that cannot work, because the
/// TV is already showing a different code.
pub const GRACE_MS: i64 = 15_000;

/// Bytes of entropy per code. 128 bits, so guessing is not a strategy even
/// against an attacker who is already on the LAN and can hammer the endpoint.
const CODE_BYTES: usize = 16;

#[derive(Debug, Clone)]
struct Issued {
    code: String,
    issued_ms: i64,
}

#[derive(Debug, Default)]
struct State {
    current: Option<Issued>,
    /// The code the TV showed until the last rotation. See [`GRACE_MS`].
    previous: Option<Issued>,
}

/// The live join code, and the one just retired.
#[derive(Debug, Default)]
pub struct JoinCodes {
    state: Mutex<State>,
}

impl JoinCodes {
    /// A fresh issuer with no code yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The code to put on the TV right now, rotating if the last one is stale.
    pub fn current(&self, now_ms: i64) -> String {
        let mut state = self.state.lock().expect("join codes mutex poisoned");

        let stale = match &state.current {
            Some(issued) => now_ms - issued.issued_ms >= ROTATE_MS,
            None => true,
        };
        if stale {
            state.previous = state.current.take();
            state.current = Some(Issued {
                code: random_code(),
                issued_ms: now_ms,
            });
        }

        state
            .current
            .as_ref()
            .expect("a code was just issued")
            .code
            .clone()
    }

    /// Spend a code. Returns false if it is unknown, expired, or already used.
    ///
    /// Deliberately does not rotate. Expiry is decided by the code's own age,
    /// so a code cannot outlive its window just because nobody was looking at
    /// the TV — which is exactly the photographed-QR case.
    pub fn redeem(&self, code: &str, now_ms: i64) -> bool {
        let mut state = self.state.lock().expect("join codes mutex poisoned");

        let matches = |slot: &Option<Issued>| {
            slot.as_ref().is_some_and(|issued| {
                issued.code == code && now_ms - issued.issued_ms < ROTATE_MS + GRACE_MS
            })
        };

        // Burn only the slot that matched: the current and retired codes are
        // independent, and redeeming one must not invalidate the other.
        if matches(&state.current) {
            state.current = None;
            return true;
        }
        if matches(&state.previous) {
            state.previous = None;
            return true;
        }
        false
    }
}

/// Bytes of entropy per phone token. Longer than a code because a code lives
/// for a minute and a token lives until the phone forgets it.
const TOKEN_BYTES: usize = 32;

/// A fresh code: 128 bits of OS entropy, hex encoded.
fn random_code() -> String {
    random_hex(CODE_BYTES)
}

/// A fresh per-person bearer token.
pub fn new_token() -> String {
    random_hex(TOKEN_BYTES)
}

fn random_hex(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    getrandom::getrandom(&mut bytes).expect("no OS entropy available");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An arbitrary fixed epoch. Every test drives time explicitly; nothing
    /// here reads the clock, which is what makes these pure.
    const T0: i64 = 1_786_937_604_000;

    #[test]
    fn a_code_is_issued_on_first_use() {
        let codes = JoinCodes::new();
        assert_eq!(codes.current(T0).len(), CODE_BYTES * 2);
    }

    #[test]
    fn the_same_code_is_shown_until_it_goes_stale() {
        let codes = JoinCodes::new();
        let first = codes.current(T0);
        assert_eq!(codes.current(T0 + ROTATE_MS - 1), first);
    }

    #[test]
    fn a_stale_code_is_replaced() {
        let codes = JoinCodes::new();
        let first = codes.current(T0);
        let second = codes.current(T0 + ROTATE_MS);
        assert_ne!(first, second);
    }

    #[test]
    fn two_issuers_do_not_produce_the_same_code() {
        assert_ne!(JoinCodes::new().current(T0), JoinCodes::new().current(T0));
    }

    #[test]
    fn a_current_code_can_be_redeemed() {
        let codes = JoinCodes::new();
        let code = codes.current(T0);
        assert!(codes.redeem(&code, T0));
    }

    // The property the whole design rests on.
    #[test]
    fn a_code_can_only_be_redeemed_once() {
        let codes = JoinCodes::new();
        let code = codes.current(T0);

        assert!(codes.redeem(&code, T0));
        assert!(
            !codes.redeem(&code, T0),
            "a burned code must not work again"
        );
    }

    #[test]
    fn an_unknown_code_is_refused() {
        let codes = JoinCodes::new();
        codes.current(T0);
        assert!(!codes.redeem("not-a-real-code", T0));
    }

    #[test]
    fn redeeming_before_any_code_is_issued_is_refused() {
        assert!(!JoinCodes::new().redeem("anything", T0));
    }

    // The scan-across-a-rotation case GRACE_MS exists for.
    #[test]
    fn the_just_retired_code_still_works_inside_the_grace_window() {
        let codes = JoinCodes::new();
        let first = codes.current(T0);
        let second = codes.current(T0 + ROTATE_MS);
        assert_ne!(first, second);

        assert!(codes.redeem(&first, T0 + ROTATE_MS + GRACE_MS - 1));
    }

    #[test]
    fn the_retired_code_stops_working_after_the_grace_window() {
        let codes = JoinCodes::new();
        let first = codes.current(T0);
        codes.current(T0 + ROTATE_MS);

        assert!(!codes.redeem(&first, T0 + ROTATE_MS + GRACE_MS));
    }

    // The photograph case. Nobody looked at the TV for an hour, so nothing
    // rotated it — expiry must not depend on someone having asked for a code.
    #[test]
    fn an_ancient_code_is_refused_even_if_it_was_never_rotated_out() {
        let codes = JoinCodes::new();
        let code = codes.current(T0);

        assert!(!codes.redeem(&code, T0 + ROTATE_MS + GRACE_MS));
    }

    #[test]
    fn redeeming_the_retired_code_leaves_the_current_one_usable() {
        let codes = JoinCodes::new();
        let first = codes.current(T0);
        let second = codes.current(T0 + ROTATE_MS);

        assert!(codes.redeem(&first, T0 + ROTATE_MS));
        assert!(
            codes.redeem(&second, T0 + ROTATE_MS),
            "burning one code must not burn the other"
        );
    }
}
