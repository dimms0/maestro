pub(crate) struct Arena<T> {
    items: Vec<Option<T>>,
    free_list: Vec<usize>,
}

impl<T> Arena<T> {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            free_list: Vec::new(),
        }
    }

    pub fn insert(&mut self, val: T) -> usize {
        if let Some(index) = self.free_list.pop() {
            // Reuse an existing hole
            self.items[index] = Some(val);
            index
        } else {
            // No holes available, grow the vector
            let index = self.items.len();
            self.items.push(Some(val));
            index
        }
    }

    pub fn remove(&mut self, index: usize) -> Option<T> {
        if index < self.items.len() {
            let item = self.items[index].take();
            if item.is_some() {
                self.free_list.push(index);
            }
            item
        } else {
            None
        }
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.items.get(index)?.as_ref()
    }

    pub fn drain(&mut self) -> impl Iterator<Item = T> {
        self.free_list.clear();
        self.items.drain(..).flatten()
    }
}

pub fn prepapre_cache_vec<T: Copy>(vec: &mut Vec<T>, len: usize, default: T) {
    if vec.len() < len {
        vec.reserve(len - vec.len());
    }
    unsafe {
        vec.set_len(len);
    }
    vec.fill(default);
}

pub fn sum_buffer(source: &[f32], target: &mut [f32]) {
    assert_eq!(source.len(), target.len(), "Buffer lengths must match");
    for (t, s) in target.iter_mut().zip(source) {
        *t += *s;
    }
}

pub(crate) struct Jitter {
    seed: u64,
    spread: f32,
}

impl Jitter {
    /// variation is a percentage (0-100)
    pub fn new(variation: f32) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        Self {
            seed,
            spread: variation.clamp(0.0, 100.0) / 100.0,
        }
    }

    pub fn active(&self) -> bool {
        self.spread > 0.0
    }

    /// base scaled by a random factor within 1 +/- spread.
    pub fn apply(&mut self, base: f32) -> f32 {
        if !self.active() {
            return base;
        }

        self.seed = self.seed.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;

        let rand_val = (z >> 40) as f32 / (1u64 << 24) as f32;
        base * (1.0 + (rand_val * 2.0 - 1.0) * self.spread)
    }
}

#[cfg(test)]
mod jitter_tests {
    use super::Jitter;

    #[test]
    fn no_variation_returns_the_base_untouched() {
        let mut j = Jitter::new(0.0);
        assert!(!j.active());
        for _ in 0..100 {
            assert_eq!(j.apply(480.0), 480.0);
        }
    }

    #[test]
    fn variation_stays_within_its_percentage() {
        let mut j = Jitter::new(25.0);
        assert!(j.active());
        let mut moved = false;
        for _ in 0..1000 {
            let v = j.apply(1000.0);
            assert!((750.0..=1250.0).contains(&v), "{v} outside +/-25%");
            moved |= v != 1000.0;
        }
        assert!(moved, "a non-zero variation must actually vary the value");
    }

    #[test]
    fn a_variation_over_100_percent_is_clamped() {
        let mut j = Jitter::new(500.0);
        for _ in 0..1000 {
            let v = j.apply(100.0);
            assert!((0.0..=200.0).contains(&v), "{v} outside +/-100%");
        }
    }
}
