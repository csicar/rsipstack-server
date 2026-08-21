//! Central registry for all metrics emitted by rsipstack-server.
//!
//! All metric names, label values, and counter initialization must be defined here.
//! [`initialize_metrics`] is called by `SipServer::new`, so applications only need to
//! install a metrics recorder before constructing a server.

use std::sync::Once;
use rsipstack::dialog::dialog::TerminatedReason;
use rsipstack::sip::StatusCode;

pub const CALLS_REJECTED_TOTAL: &str = "rsipstack_server.calls.rejected_total";
pub const CALLS_ACCEPTED_TOTAL: &str = "rsipstack_server.calls.accepted_total";
pub const CALLS_ACTIVE: &str = "rsipstack_server.calls.active";
pub const CALLS_DIALOG_NOT_FOUND_TOTAL: &str = "rsipstack_server.calls.dialog_not_found_total";
pub const CALLS_TERMINATED_TOTAL: &str = "rsipstack_server.calls.terminated_total";
pub const PORTS_CAPACITY: &str = "rsipstack_server.ports.capacity";
pub const PORTS_ALLOCATION_ATTEMPTS: &str = "rsipstack_server.ports.allocation_attempts";
pub const PORTS_ALLOCATION_FAILURES: &str = "rsipstack_server.ports.allocation_failures";
pub const DRAIN_ACTIVE: &str = "rsipstack_server.drain_active";

#[derive(Debug, Clone, Copy)]
pub enum RejectReason {
    SdpOfferInvalid,
    RtpPortPoolExhausted,
    UdpConnectFailed,
}

impl RejectReason {
    fn as_str(&self) -> &'static str {
        match self {
            Self::SdpOfferInvalid => "sdp_offer_invalid",
            Self::RtpPortPoolExhausted => "rtp_port_pool_exhausted",
            Self::UdpConnectFailed => "udp_connect_failed",
        }
    }
}

/// Maps a [`TerminatedReason`] to its metric label, deliberately dropping the `StatusCode`
/// payload carried by `ProxyError`, `UacOther`, and `UasOther`. That payload can embed an
/// arbitrary, remote-controlled reason phrase (`StatusCode::Other(u16, String)`), which
/// would otherwise let a peer inject unbounded label values into this metric. The specific
/// status code is still available in the log line emitted alongside this counter.
///
/// This match has no wildcard arm, so if rsipstack ever adds a new `TerminatedReason`
/// variant, this fails to *compile* until it's given a label here.
fn terminated_reason_label(reason: &TerminatedReason) -> &'static str {
    match reason {
        TerminatedReason::Timeout => "Timeout",
        TerminatedReason::UacCancel => "UacCancel",
        TerminatedReason::UacBye => "UacBye",
        TerminatedReason::UasBye => "UasBye",
        TerminatedReason::UacBusy => "UacBusy",
        TerminatedReason::UasBusy => "UasBusy",
        TerminatedReason::UasDecline => "UasDecline",
        TerminatedReason::ProxyError(_) => "ProxyError",
        TerminatedReason::ProxyAuthRequired => "ProxyAuthRequired",
        TerminatedReason::UacOther(_) => "UacOther",
        TerminatedReason::UasOther(_) => "UasOther",
    }
}

/// One sample per [`TerminatedReason`] variant, used only to zero-init every label in
/// [`initialize_metrics`]. The `StatusCode` payloads are ignored by [`terminated_reason_label`],
/// so their value here is arbitrary. Keep in sync with `terminated_reason_label`.
const ALL_TERMINATED_REASONS: [TerminatedReason; 11] = [
    TerminatedReason::Timeout,
    TerminatedReason::UacCancel,
    TerminatedReason::UacBye,
    TerminatedReason::UasBye,
    TerminatedReason::UacBusy,
    TerminatedReason::UasBusy,
    TerminatedReason::UasDecline,
    TerminatedReason::ProxyError(StatusCode::OK),
    TerminatedReason::ProxyAuthRequired,
    TerminatedReason::UacOther(StatusCode::OK),
    TerminatedReason::UasOther(StatusCode::OK),
];

pub fn calls_rejected(reason: RejectReason) -> metrics::Counter {
    metrics::counter!(
        description: "Number of rejected incoming SIP calls",
        CALLS_REJECTED_TOTAL,
        "reason" => reason.as_str()
    )
}

pub fn calls_accepted() -> metrics::Counter {
    metrics::counter!(
        description: "Number of successfully accepted SIP calls",
        CALLS_ACCEPTED_TOTAL
    )
}

pub fn calls_active() -> metrics::Gauge {
    metrics::gauge!(
        unit: metrics::Unit::Count,
        description: "Number of currently active SIP calls",
        CALLS_ACTIVE
    )
}

pub fn calls_dialog_not_found() -> metrics::Counter {
    metrics::counter!(
        description: "Number of in-dialog requests for which no matching dialog was found",
        CALLS_DIALOG_NOT_FOUND_TOTAL
    )
}

pub fn calls_terminated(reason: &TerminatedReason) -> metrics::Counter {
    metrics::counter!(
        description: "Number of terminated SIP calls",
        CALLS_TERMINATED_TOTAL,
        "reason" => terminated_reason_label(reason)
    )
}

pub fn ports_capacity() -> metrics::Gauge {
    metrics::gauge!(
        unit: metrics::Unit::Count,
        description: "Upper bound of allocatable RTP/RTCP port pairs. OS may have some ports bound.",
        PORTS_CAPACITY
    )
}

pub fn ports_allocation_attempts() -> metrics::Histogram {
    metrics::histogram!(
        unit: metrics::Unit::Count,
        description: "Number of attempts before free port pair was found",
        PORTS_ALLOCATION_ATTEMPTS
    )
}

pub fn ports_allocation_failures() -> metrics::Counter {
    metrics::counter!(
        description: "Number of times the RTP port pool was exhausted and no socket pair could be bound",
        PORTS_ALLOCATION_FAILURES
    )
}

pub fn drain_active() -> metrics::Gauge {
    metrics::gauge!(
        description: "Whether the server is currently draining (1) or not (0)",
        DRAIN_ACTIVE
    )
}

/// Pre-registers metrics with a value of zero so they appear in scrapes before the
/// events they track have ever occurred. Called by `SipServer::new`; a metrics
/// recorder must be installed before that point for these values to be recorded.
///
/// Runs at most once per process. `calls.active` is a process-global gauge shared by
/// every server, so a second `SipServer::new` must not reset it while the first server
/// still has live calls — that would make the gauge drift negative as those calls end.
///
/// Gauges that are set by the code owning their lifecycle at the correct point in
/// time (`ports.capacity` on `RtpPortRange::new`, `drain_active` when a server starts
/// serving) are left alone here to avoid clobbering a value set before this runs.
pub fn initialize_metrics() {
    for reason in [
        RejectReason::SdpOfferInvalid,
        RejectReason::RtpPortPoolExhausted,
        RejectReason::UdpConnectFailed,
    ] {
        calls_rejected(reason).absolute(0);
    }
    calls_accepted().absolute(0);
    calls_active().set(0);
    calls_dialog_not_found().absolute(0);
    for reason in &ALL_TERMINATED_REASONS {
        calls_terminated(reason).absolute(0);
    }
    ports_allocation_failures().absolute(0);
}

pub struct ScopedGauge {
    gauge: metrics::Gauge,
}

impl ScopedGauge {
    pub fn new(gauge: metrics::Gauge) -> Self {
        gauge.increment(1);
        ScopedGauge { gauge }
    }
}

impl Drop for ScopedGauge {
    fn drop(&mut self) {
        self.gauge.decrement(1);
    }
}
