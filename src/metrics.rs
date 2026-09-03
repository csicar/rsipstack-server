//! Central registry for all metrics emitted by rsipstack-server.
//!
//! All metric names, label values, and counter initialization must be defined here.
//! [`ensure_initialized`] is called by `SipServer::new`, so applications only need to
//! install a metrics recorder before constructing a server.
//!
//! Every labelled metric gets its label values from an enum deriving [`strum::EnumIter`],
//! so the set of labels is defined exactly once: at the enum. Nothing here keeps a
//! separate hand-written list of variants.

use std::{sync::Once, time::Duration};

use metrics::Histogram;
use rsipstack::dialog::dialog::TerminatedReason;
use strum::{EnumIter, IntoEnumIterator, IntoStaticStr};
use tokio::time::Instant;

pub const CALLS_REJECTED_TOTAL: &str = "rsipstack_server.calls.rejected_total";
pub const CALLS_ACCEPTED_TOTAL: &str = "rsipstack_server.calls.accepted_total";
pub const CALLS_ACTIVE: &str = "rsipstack_server.calls.active";
pub const CALLS_DIALOG_NOT_FOUND_TOTAL: &str = "rsipstack_server.calls.dialog_not_found_total";
pub const CALLS_TERMINATED_TOTAL: &str = "rsipstack_server.calls.terminated_total";
pub const PORTS_CAPACITY: &str = "rsipstack_server.ports.capacity";
pub const PORTS_ALLOCATION_ATTEMPTS: &str = "rsipstack_server.ports.allocation_attempts";
pub const PORTS_ALLOCATION_FAILURES: &str = "rsipstack_server.ports.allocation_failures";
pub const DRAIN_ACTIVE: &str = "rsipstack_server.drain_active";
pub const RTP_PACKETS_SENT_TOTAL: &str = "rsipstack_server.rtp.packets_sent_total";
pub const RTP_SEND_ERRORS_TOTAL: &str = "rsipstack_server.rtp.send_errors_total";
pub const RTP_SEND_TIMING_DEVIATION_SECONDS: &str =
    "rsipstack_server.rtp.send_timing_deviation_seconds";

/// Label values for the `reason` label on [`CALLS_REJECTED_TOTAL`].
#[derive(Debug, Clone, Copy, EnumIter, IntoStaticStr)]
pub enum RejectReason {
    SdpOfferInvalid,
    RtpPortPoolExhausted,
    UdpConnectFailed,
}

/// Label values for the `reason` label on [`CALLS_TERMINATED_TOTAL`].
///
/// Mirrors [`TerminatedReason`] with the `StatusCode` payloads of `ProxyError`, `UacOther`,
/// and `UasOther` deliberately dropped. That payload can embed an arbitrary,
/// remote-controlled reason phrase (`StatusCode::Other(u16, String)`), which would otherwise
/// let a peer inject unbounded label values into this metric. Nothing in this crate reads
/// the status code apart from the `"Call terminated"` log line emitted alongside this
/// counter, which still prints it in full.
///
/// A local, payload-free mirror is also what makes the label set enumerable: `EnumIter`
/// cannot be derived on `TerminatedReason` itself, since it is a foreign type.
#[derive(Debug, Clone, Copy, EnumIter, IntoStaticStr)]
pub enum TerminatedLabel {
    Timeout,
    UacCancel,
    UacBye,
    UasBye,
    UacBusy,
    UasBusy,
    UasDecline,
    ProxyError,
    ProxyAuthRequired,
    UacOther,
    UasOther,
}

impl From<&TerminatedReason> for TerminatedLabel {
    /// This match has no wildcard arm, so if rsipstack ever adds a new `TerminatedReason`
    /// variant, this fails to *compile* until it is given a label here.
    fn from(reason: &TerminatedReason) -> Self {
        match reason {
            TerminatedReason::Timeout => Self::Timeout,
            TerminatedReason::UacCancel => Self::UacCancel,
            TerminatedReason::UacBye => Self::UacBye,
            TerminatedReason::UasBye => Self::UasBye,
            TerminatedReason::UacBusy => Self::UacBusy,
            TerminatedReason::UasBusy => Self::UasBusy,
            TerminatedReason::UasDecline => Self::UasDecline,
            TerminatedReason::ProxyError(_) => Self::ProxyError,
            TerminatedReason::ProxyAuthRequired => Self::ProxyAuthRequired,
            TerminatedReason::UacOther(_) => Self::UacOther,
            TerminatedReason::UasOther(_) => Self::UasOther,
        }
    }
}

pub fn calls_rejected(reason: RejectReason) -> metrics::Counter {
    metrics::counter!(
        description: "Number of rejected incoming SIP calls",
        CALLS_REJECTED_TOTAL,
        "reason" => <&'static str>::from(reason)
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

/// Accepts a `&TerminatedReason` at the call site and a bare [`TerminatedLabel`] from
/// [`ensure_initialized`], which only has labels to iterate over and no reason to construct.
pub fn calls_terminated(reason: impl Into<TerminatedLabel>) -> metrics::Counter {
    metrics::counter!(
        description: "Number of terminated SIP calls",
        CALLS_TERMINATED_TOTAL,
        "reason" => <&'static str>::from(reason.into())
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

pub fn rtp_packets_sent() -> metrics::Counter {
    metrics::counter!(
        description: "Number of RTP packets successfully sent",
        RTP_PACKETS_SENT_TOTAL
    )
}

pub fn rtp_send_errors() -> metrics::Counter {
    metrics::counter!(
        description: "Number of RTP packet send failures",
        RTP_SEND_ERRORS_TOTAL
    )
}

/// Records how far the real interval between two events drifts from an expected
/// fixed cadence.
///
/// Each [`record`](Self::record) call measures the time since the previous call and
/// emits `elapsed - expected_interval` (in seconds) to the wrapped histogram: zero
/// means the event landed exactly on schedule, a positive sample means it was late,
/// and a negative sample means it fired early. The very first call has no predecessor
/// to compare against, so it only arms the clock and records nothing.
///
/// This is stateful — it owns the `last_tick` timestamp — so a single instance must be
/// kept for the lifetime of the thing being measured and its `record` calls must all
/// come from the same source of ticks. For RTP sends this means one instance per send
/// task, ticked once per successful packet; see [`rtp_send_timing_deviation`].
pub struct TimingDeviationMetric {
    /// The cadence the events are supposed to keep; every sample is measured relative to it.
    expected_interval: Duration,
    /// Timestamp of the previous [`record`](Self::record) call, or `None` before the first tick.
    last_tick: Option<Instant>,
    /// Histogram the per-tick deviation (in seconds) is written to.
    metric: Histogram,
}

impl TimingDeviationMetric {
    /// Creates a metric that reports deviation from `expected_interval`, writing samples
    /// into `metric`.
    ///
    /// Pass a [`Histogram`] obtained from your own `metrics::histogram!` call (whose unit
    /// should be seconds, since that is what [`record`](Self::record) emits). Within this
    /// crate, [`rtp_send_timing_deviation`] wires up the right histogram and interval for
    /// RTP sends; external callers construct instances through this constructor directly.
    pub fn new(metric: Histogram, expected_interval: Duration) -> Self {
        Self {
            expected_interval,
            metric,
            last_tick: None,
        }
    }

    /// Records one tick.
    ///
    /// If a previous tick exists, emits `elapsed_since_previous - expected_interval`
    /// (seconds) to the histogram; the first call after construction only stores the
    /// current time and records nothing. Either way the internal clock advances to now,
    /// so consecutive samples measure back-to-back gaps and a stall surfaces as a single
    /// large sample rather than being smeared across several.
    pub fn record(&mut self) {
        let now = Instant::now();
        if let Some(prev) = self.last_tick {
            let elapsed_time = now - prev;
            let deviation = elapsed_time.as_secs_f64() - self.expected_interval.as_secs_f64();
            self.metric.record(deviation);
        }
        self.last_tick = Some(now);
    }
}

/// Deviation of the time since the previous successful send from the expected
/// 20ms (`a=ptime:20`) send interval. Recorded regardless of magnitude, so a
/// stall shows up as a single large sample rather than being hidden by any
/// catch-up behavior.
pub fn rtp_send_timing_deviation(expected_interval: Duration) -> TimingDeviationMetric {
    let metric = metrics::histogram!(
        unit: metrics::Unit::Seconds,
        description: "Deviation from the expected 20ms interval between RTP packet sends",
        RTP_SEND_TIMING_DEVIATION_SECONDS
    );

    TimingDeviationMetric::new(metric, expected_interval)
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
pub fn ensure_initialized() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        for reason in RejectReason::iter() {
            calls_rejected(reason).absolute(0);
        }
        calls_accepted().absolute(0);
        calls_active().set(0);
        calls_dialog_not_found().absolute(0);
        for label in TerminatedLabel::iter() {
            calls_terminated(label).absolute(0);
        }
        ports_allocation_failures().absolute(0);
        rtp_packets_sent().absolute(0);
        rtp_send_errors().absolute(0);
    });
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
