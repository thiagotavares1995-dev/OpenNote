//! Which notes can matter at a moment. A piece has thousands of notes but only a few dozen are
//! on screen, sounding or throwing sparks at any time; this index lists, for each slice of time,
//! the notes whose span touches it, so a system visits those and not the whole piece.
//!
//! The spans are generous and the callers keep their own exact tests, and the notes come back
//! in their original order, so the result is the same as a pass over every note.

/// Seconds of time per bucket.
const BUCKET_SECONDS: f64 = 0.25;

/// Added to both ends of every span, far more than any rounding in the callers' own tests.
const MARGIN_SECONDS: f64 = 0.05;

/// Buckets of time, each listing in increasing order the items whose span touches it.
#[derive(Debug, Clone, Default)]
pub struct TimeIndex {
    origin: f64,
    buckets: Vec<Vec<u32>>,
}

impl TimeIndex {
    /// `spans[i]` is the closed interval of time in which item `i` may matter. An empty or
    /// non-finite span leaves the item out.
    pub fn new(spans: impl IntoIterator<Item = (f64, f64)>) -> Self {
        let spans: Vec<(f64, f64)> = spans.into_iter().collect();
        let usable = |&(from, to): &(f64, f64)| from.is_finite() && to.is_finite() && from <= to;
        let Some(origin) = spans.iter().filter(|s| usable(s)).map(|s| s.0).reduce(f64::min) else {
            return Self::default();
        };
        let origin = origin - MARGIN_SECONDS;
        let end = spans.iter().filter(|s| usable(s)).map(|s| s.1).fold(origin, f64::max) + MARGIN_SECONDS;
        let mut index = Self { origin, buckets: vec![Vec::new(); Self::slot(origin, end) + 1] };
        for (item, span) in spans.iter().enumerate() {
            if !usable(span) {
                continue;
            }
            let first = Self::slot(origin, span.0 - MARGIN_SECONDS);
            let last = Self::slot(origin, span.1 + MARGIN_SECONDS);
            for bucket in &mut index.buckets[first..=last] {
                bucket.push(item as u32);
            }
        }
        index
    }

    fn slot(origin: f64, time: f64) -> usize {
        ((time - origin) / BUCKET_SECONDS).floor().max(0.0) as usize
    }

    /// The items that may matter at `time`, in increasing order.
    pub fn at(&self, time: f64) -> &[u32] {
        if !time.is_finite() || time < self.origin {
            return &[];
        }
        self.buckets.get(Self::slot(self.origin, time)).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_whose_span_covers_a_moment_is_listed_in_order() {
        let spans = [(0.0, 1.0), (0.5, 0.6), (2.0, 9.0), (3.0, 2.0), (f64::NAN, 1.0), (-4.0, -3.5)];
        let index = TimeIndex::new(spans);
        for step in -500..1100 {
            let time = f64::from(step) / 100.0;
            let listed = index.at(time);
            assert!(listed.windows(2).all(|w| w[0] < w[1]), "in order at {time}");
            for (item, &(from, to)) in spans.iter().enumerate() {
                if from <= time && time <= to {
                    assert!(listed.contains(&(item as u32)), "item {item} missing at {time}");
                }
            }
            assert!(!listed.contains(&3) && !listed.contains(&4), "empty spans are left out");
        }
        assert!(index.at(100.0).is_empty() && index.at(-100.0).is_empty() && index.at(f64::NAN).is_empty());
        assert!(TimeIndex::new([]).at(0.0).is_empty());
    }
}
