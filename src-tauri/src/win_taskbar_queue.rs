//! Bounded, coalescing taskbar mailbox; one event-thread producer and one worker.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub const CAPACITY: usize = 32;
const DIRTY: u64 = 1 << 32;
const REFRESH: u64 = 1 << 33;
const PRESENT: u64 = 1 << 34;
const OVERLAY: u64 = 1 << 35;
const GENERATION_SHIFT: u32 = 36;
pub const MAX_GENERATION: usize = (1 << (64 - GENERATION_SHIFT)) - 1;

#[derive(Clone, Copy, Debug)]
pub struct Request {
    pub index: usize,
    pub hwnd: usize,
    state: u64,
}

impl Request {
    pub fn generation(self) -> usize {
        (self.state >> GENERATION_SHIFT) as usize
    }

    pub fn present(self) -> bool {
        self.state & PRESENT != 0
    }

    pub fn refresh(self) -> bool {
        self.state & REFRESH != 0
    }

    pub fn overlay_count(self) -> Option<u32> {
        (self.state & OVERLAY != 0).then_some(self.state as u32)
    }
}

struct Slot {
    hwnd: AtomicUsize,
    state: AtomicU64,
}

pub struct Mailbox {
    slots: [Slot; CAPACITY],
}

impl Mailbox {
    pub const fn new() -> Self {
        Self {
            slots: [const {
                Slot {
                    hwnd: AtomicUsize::new(0),
                    state: AtomicU64::new(0),
                }
            }; CAPACITY],
        }
    }

    /// Only the event thread installs/recycles slots; generations never repeat.
    pub fn install(&self, index: usize, hwnd: usize, generation: usize, present: bool) {
        assert!((1..=MAX_GENERATION).contains(&generation));
        let slot = &self.slots[index];
        slot.state.store(0, Ordering::Release);
        slot.hwnd.store(hwnd, Ordering::Release);
        slot.state.store(
            ((generation as u64) << GENERATION_SHIFT) | if present { PRESENT } else { 0 },
            Ordering::Release,
        );
    }

    pub fn current(&self, index: usize) -> Option<Request> {
        let slot = &self.slots[index];
        self.snapshot(index, slot.state.load(Ordering::Acquire))
    }

    fn snapshot(&self, index: usize, state: u64) -> Option<Request> {
        let slot = &self.slots[index];
        let request = Request {
            index,
            hwnd: slot.hwnd.load(Ordering::Acquire),
            state,
        };
        let generation = slot.state.load(Ordering::Acquire) >> GENERATION_SHIFT;
        (generation != 0 && generation == state >> GENERATION_SHIFT).then_some(request)
    }

    pub fn set_tab(&self, index: usize, present: bool, overlay_count: Option<u32>) {
        self.slots[index]
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                Some(
                    (state & !((u32::MAX as u64) | PRESENT | OVERLAY))
                        | DIRTY
                        | REFRESH
                        | if present { PRESENT } else { 0 }
                        | overlay_count.map_or(0, |count| OVERLAY | u64::from(count)),
                )
            })
            .ok();
    }

    pub fn set_overlay(&self, index: usize, count: u32) {
        self.slots[index]
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                Some((state & !(u32::MAX as u64)) | DIRTY | OVERLAY | u64::from(count))
            })
            .ok();
    }

    pub fn take(&self, index: usize) -> Option<Request> {
        let state = self.slots[index]
            .state
            .fetch_and(!(DIRTY | REFRESH), Ordering::AcqRel);
        if state & DIRTY == 0 {
            return None;
        }
        self.snapshot(index, state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mailbox() -> Mailbox {
        let mailbox = Mailbox::new();
        mailbox.install(0, 100, 1, true);
        mailbox
    }

    #[test]
    fn rapid_hide_show_keeps_only_final_presence() {
        let mailbox = mailbox();
        for _ in 0..10_000 {
            mailbox.set_tab(0, false, None);
            mailbox.set_tab(0, true, Some(7));
        }
        let request = mailbox.take(0).unwrap();
        assert!(request.present() && request.refresh());
        assert_eq!(request.overlay_count(), Some(7));
        assert!(mailbox.take(0).is_none());
        mailbox.set_tab(0, false, None);
        assert!(!mailbox.take(0).unwrap().present());
    }

    #[test]
    fn overlay_does_not_erase_pending_registration_or_register_again() {
        let mailbox = mailbox();
        mailbox.set_tab(0, true, Some(8));
        mailbox.set_overlay(0, 0);
        let request = mailbox.take(0).unwrap();
        assert!(request.refresh());
        assert_eq!(request.overlay_count(), Some(0));
        mailbox.set_overlay(0, 4);
        assert!(!mailbox.take(0).unwrap().refresh());
    }

    #[test]
    fn in_flight_show_is_followed_by_final_hide() {
        let mailbox = mailbox();
        mailbox.set_tab(0, true, Some(2));
        let in_flight = mailbox.take(0).unwrap();
        mailbox.set_tab(0, false, None);
        assert!(in_flight.present());
        assert!(!mailbox.current(0).unwrap().present());
        assert!(!mailbox.take(0).unwrap().present());
    }

    #[test]
    fn badge_change_during_registration_remains_pending() {
        let mailbox = mailbox();
        mailbox.set_tab(0, true, Some(2));
        mailbox.take(0).unwrap();
        mailbox.set_overlay(0, 9);
        assert_eq!(mailbox.current(0).unwrap().overlay_count(), Some(9));
        assert_eq!(mailbox.take(0).unwrap().overlay_count(), Some(9));
    }

    #[test]
    fn tools_are_separate_and_receive_no_main_badge() {
        let mailbox = mailbox();
        mailbox.install(1, 200, 2, false);
        mailbox.set_tab(0, true, Some(9));
        mailbox.set_tab(1, false, None);
        assert_eq!(mailbox.take(0).unwrap().overlay_count(), Some(9));
        let tool = mailbox.take(1).unwrap();
        assert!(!tool.present());
        assert_eq!(tool.overlay_count(), None);
    }

    #[test]
    fn recycled_handle_invalidates_old_generation() {
        let mailbox = mailbox();
        mailbox.set_tab(0, true, Some(2));
        let old = mailbox.take(0).unwrap();
        mailbox.install(0, 100, 2, false);
        let current = mailbox.current(0).unwrap();
        assert_eq!(old.hwnd, current.hwnd);
        assert_ne!(old.generation(), current.generation());
        assert_eq!(current.overlay_count(), None);
        assert!(mailbox.take(0).is_none());
    }

    #[test]
    fn enqueue_stays_fast_while_consumer_is_blocked_in_com() {
        use std::sync::mpsc;
        use std::time::Duration;

        let mailbox = mailbox();
        mailbox.install(1, 200, 2, false);
        mailbox.set_tab(0, true, Some(1));
        std::thread::scope(|scope| {
            let mailbox = &mailbox;
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let consumer = scope.spawn(move || {
                let in_flight = mailbox.take(0).unwrap();
                entered_tx.send(()).unwrap();
                // Simulate a COM call that cannot finish until the test releases it.
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                assert_eq!(in_flight.overlay_count(), Some(1));
                let main = mailbox.take(0).unwrap();
                assert!(!main.present());
                assert_eq!(main.overlay_count(), Some(0));
                let pet = mailbox.take(1).unwrap();
                assert!(!pet.present());
                assert_eq!(pet.overlay_count(), None);
                assert!(mailbox.take(0).is_none());
            });
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let wake = consumer.thread().clone();
            let (finished_tx, finished_rx) = mpsc::channel();
            scope.spawn(move || {
                for count in 0..10_000 {
                    mailbox.set_tab(0, count % 2 == 0, Some(count));
                    mailbox.set_tab(1, false, None);
                    wake.unpark();
                }
                mailbox.set_overlay(0, 0);
                wake.unpark();
                finished_tx.send(()).unwrap();
            });
            let finished = finished_rx.recv_timeout(Duration::from_secs(1));
            // Release even on timeout, so a failed responsiveness assertion cannot hang.
            release_tx.send(()).unwrap();
            assert!(finished.is_ok(), "enqueue waited for the blocked consumer");
        });
    }

    #[test]
    fn enqueue_after_scan_before_park_retains_wake_and_final_state() {
        use std::sync::atomic::AtomicBool;
        use std::sync::mpsc;
        use std::time::Duration;

        let mailbox = mailbox();
        let may_park = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let mailbox = &mailbox;
            let may_park = &may_park;
            let (scanned_tx, scanned_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel();
            let consumer = scope.spawn(move || {
                assert!(mailbox.take(0).is_none());
                scanned_tx.send(()).unwrap();
                // Do not use another parking primitive: it could consume the permit.
                while !may_park.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                std::thread::park();
                let final_request = mailbox.take(0).unwrap();
                assert!(final_request.present() && final_request.refresh());
                assert_eq!(final_request.overlay_count(), Some(0));
                done_tx.send(()).unwrap();
            });
            scanned_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            mailbox.set_tab(0, false, None);
            mailbox.set_tab(0, true, Some(8));
            mailbox.set_overlay(0, 0);
            consumer.thread().unpark();
            may_park.store(true, Ordering::Release);
            let done = done_rx.recv_timeout(Duration::from_secs(1));
            consumer.thread().unpark();
            assert!(done.is_ok(), "wake sent before park was lost");
        });
    }

    #[test]
    fn window_entrypoints_dispatch_before_resolving_hwnd() {
        let shell = include_str!("win_shell.rs");
        for name in [
            "strip_overlay_native_menu",
            "ensure_main_window_shell_integration",
            "set_overlay_skip_taskbar",
            "set_main_window_skip_taskbar",
            "attach_webview_keyboard_focus",
        ] {
            let start = shell.find(&format!("pub fn {name}(")).unwrap();
            let body = &shell[start..start + shell[start..].find("\n}").unwrap()];
            assert!(body.contains("on_window_thread(window,"), "{name}");
            assert!(!body.contains(".hwnd()"), "{name}");
        }
        let start = shell.find("fn on_window_thread(").unwrap();
        let body = &shell[start..start + shell[start..].find("\n}").unwrap()];
        assert!(body.find("run_on_main_thread(").unwrap() < body.find(".hwnd()").unwrap());
    }

    #[test]
    fn concurrent_drain_preserves_final_update() {
        let mailbox = mailbox();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..10_000 {
                    mailbox.take(0);
                }
            });
            for count in 0..10_000 {
                mailbox.set_tab(0, count % 2 == 0, Some(count));
            }
        });
        let current = mailbox.current(0).unwrap();
        assert!(!current.present());
        assert_eq!(current.overlay_count(), Some(9_999));
    }
}
