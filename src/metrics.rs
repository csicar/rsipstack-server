pub struct GaugeGuard {
    gauge: metrics::Gauge,
}

impl GaugeGuard {
    pub fn new(gauge: metrics::Gauge) -> Self {
        gauge.increment(1);
        GaugeGuard { gauge }
    }
}

impl Drop for GaugeGuard {
    fn drop(&mut self) {
        self.gauge.decrement(1);
    }
}
