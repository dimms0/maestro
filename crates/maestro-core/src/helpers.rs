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

pub fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
