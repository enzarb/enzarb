use enzarb_proto::*;
use proptest::prelude::*;

#[test]
fn sender_cannot_exceed_credit() {
    let mut fc = FlowControl::new(100, 100);
    assert_eq!(fc.sendable(1000), 100);
    fc.on_send(60).unwrap();
    assert_eq!(fc.sendable(1000), 40);
    assert_eq!(
        fc.on_send(41),
        Err(FlowError::InsufficientCredit {
            needed: 41,
            available: 40
        })
    );
    assert_eq!(fc.send_credit(), 40, "failed send must not consume credit");
    fc.on_send(40).unwrap();
    assert_eq!(fc.sendable(1), 0);
}

#[test]
fn window_adds_credit_and_is_bounded() {
    let mut fc = FlowControl::new(0, 100);
    assert_eq!(fc.sendable(10), 0);
    fc.on_window(10).unwrap();
    assert_eq!(fc.sendable(100), 10);
    fc.on_window(MAX_CREDIT - 10).unwrap();
    assert_eq!(fc.on_window(1), Err(FlowError::CreditOverflow));
}

#[test]
fn receiver_rejects_overrun_and_regrants() {
    let mut fc = FlowControl::new(0, 100); // threshold 50
    fc.on_data(40).unwrap();
    assert_eq!(
        fc.on_data(61),
        Err(FlowError::Overrun {
            got: 61,
            granted: 60
        })
    );
    assert_eq!(fc.release(40).unwrap(), None, "below threshold");
    assert_eq!(fc.flush_grant(), Some(40));
}

#[test]
fn release_batches_until_threshold() {
    let mut fc = FlowControl::new(0, 100); // threshold 50
    fc.on_data(100).unwrap();
    assert_eq!(fc.recv_granted(), 0);
    assert_eq!(fc.release(30).unwrap(), None);
    assert_eq!(fc.release(20).unwrap(), Some(50));
    assert_eq!(fc.recv_granted(), 50);
    assert_eq!(fc.release(50).unwrap(), Some(50));
    assert_eq!(fc.recv_granted(), 100);
    assert_eq!(fc.release(1), Err(FlowError::ReleaseUnderflow));
}

#[test]
fn flush_grant_forces_small_grants() {
    let mut fc = FlowControl::new(0, 100);
    fc.on_data(10).unwrap();
    assert_eq!(fc.release(10).unwrap(), None);
    assert_eq!(fc.flush_grant(), Some(10));
    assert_eq!(fc.flush_grant(), None);
}

#[test]
fn close_semantics() {
    let mut fc = FlowControl::default();
    assert!(!fc.is_closed());
    fc.close_local().unwrap();
    assert!(fc.is_local_closed() && !fc.is_closed());
    assert_eq!(fc.on_send(1), Err(FlowError::LocalClosed));
    assert_eq!(fc.sendable(10), 0);
    assert_eq!(fc.close_local(), Err(FlowError::AlreadyClosed));
    fc.on_window(10).unwrap(); // late WINDOW is ignored, not an error
    assert_eq!(fc.send_credit(), 0);
    // receiving still works after local half-close
    fc.on_data(5).unwrap();
    fc.on_close_remote();
    assert!(fc.is_closed());
    assert_eq!(fc.on_data(1), Err(FlowError::DataAfterClose));
    // releasing buffered data after peer close grants nothing
    assert_eq!(fc.release(5).unwrap(), None);
    assert_eq!(fc.flush_grant(), None);
}

proptest! {
    /// Wire two endpoints together under random scheduling. A never sends
    /// more than allowed, B never sees an overrun, and in-flight + buffered
    /// bytes never exceed the initial window.
    #[test]
    fn simulated_link_respects_credit(
        window in 1u32..5000,
        ops in prop::collection::vec((0u8..3, 1usize..3000), 1..200),
    ) {
        let mut a = FlowControl::new(window, window);
        let mut b = FlowControl::new(window, window);
        let mut in_flight: std::collections::VecDeque<usize> = Default::default();
        let mut grants: std::collections::VecDeque<u32> = Default::default();
        let mut unreleased = 0usize;
        let mut total_sent = 0u64;
        let mut total_granted = window as u64;
        for (op, n) in ops {
            match op {
                0 => { // A sends as much as allowed up to n
                    let k = a.sendable(n);
                    if k > 0 {
                        a.on_send(k).unwrap();
                        total_sent += k as u64;
                        in_flight.push_back(k);
                    }
                    prop_assert!(a.on_send(a.sendable(usize::MAX) + 1).is_err());
                }
                1 => { // deliver to B
                    if let Some(k) = in_flight.pop_front() {
                        b.on_data(k).unwrap();
                        unreleased += k;
                    }
                }
                _ => { // B app consumes, maybe grants; A receives grants
                    let k = n.min(unreleased);
                    unreleased -= k;
                    if let Some(g) = b.release(k).unwrap() { grants.push_back(g); }
                    if let Some(g) = grants.pop_front() {
                        total_granted += g as u64;
                        a.on_window(g).unwrap();
                    }
                }
            }
            prop_assert!(total_sent <= total_granted);
            let buffered: usize = in_flight.iter().sum::<usize>() + unreleased;
            prop_assert!(buffered as u64 <= window as u64);
        }
    }
}
