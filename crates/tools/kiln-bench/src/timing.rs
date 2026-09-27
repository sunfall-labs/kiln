//! Order statistics over a set of timed iterations.
//!
//! A single sample is not a measurement. Everything reported goes through here so that the
//! output always carries a spread and an iteration count alongside the headline number.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Samples {
    /// Ascending, in milliseconds.
    sorted_ms: Vec<f64>,
}

impl Samples {
    /// Panics if `durations` is empty; a scenario that produced no samples is a harness bug,
    /// not a runtime condition.
    pub fn from(durations: &[Duration]) -> Samples {
        assert!(!durations.is_empty(), "no samples");
        let mut sorted_ms: Vec<f64> = durations.iter().map(|d| d.as_secs_f64() * 1e3).collect();
        sorted_ms.sort_by(f64::total_cmp);
        Samples { sorted_ms }
    }

    pub fn len(&self) -> usize {
        self.sorted_ms.len()
    }

    pub fn min(&self) -> f64 {
        self.sorted_ms[0]
    }

    pub fn max(&self) -> f64 {
        self.sorted_ms[self.sorted_ms.len() - 1]
    }

    /// Reported instead of a mean: one descheduled iteration should not move the headline.
    pub fn median(&self) -> f64 {
        let n = self.sorted_ms.len();
        if n % 2 == 1 {
            self.sorted_ms[n / 2]
        } else {
            (self.sorted_ms[n / 2 - 1] + self.sorted_ms[n / 2]) / 2.0
        }
    }

    /// Nearest-rank percentile, `p` in `0.0..=1.0`.
    pub fn percentile(&self, p: f64) -> f64 {
        let n = self.sorted_ms.len();
        let rank = (p * n as f64).ceil().max(1.0) as usize;
        self.sorted_ms[rank.min(n) - 1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(values: &[u64]) -> Samples {
        let d: Vec<Duration> = values.iter().map(|&v| Duration::from_millis(v)).collect();
        Samples::from(&d)
    }

    #[test]
    fn order_statistics_ignore_input_order() {
        let a = ms(&[10, 1, 5, 3, 9]);
        assert_eq!(a.min(), 1.0);
        assert_eq!(a.max(), 10.0);
        assert_eq!(a.median(), 5.0);
        assert_eq!(a.len(), 5);
    }

    #[test]
    fn median_of_even_count_averages_the_middle_pair() {
        assert_eq!(ms(&[2, 4, 6, 8]).median(), 5.0);
    }

    #[test]
    fn percentiles_use_nearest_rank_and_stay_in_bounds() {
        let s = ms(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(s.percentile(0.9), 9.0);
        assert_eq!(s.percentile(0.0), 1.0);
        assert_eq!(s.percentile(1.0), 10.0);
    }

    #[test]
    fn a_single_sample_is_its_own_every_statistic() {
        let s = ms(&[7]);
        assert_eq!(
            (s.min(), s.median(), s.percentile(0.9), s.max()),
            (7.0, 7.0, 7.0, 7.0)
        );
    }
}
