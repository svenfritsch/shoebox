//! Nearest neighbours of face embeddings without comparing every pair
//! (phase 5c-2): an inverted-file index. The embeddings are split into
//! cells around k-means centres (about √n of them); a search compares the
//! query with the centres and then only with the embeddings in the closest
//! cells. Up to `EXACT_MAX` embeddings it compares with all of them.
//!
//! Embeddings are L2-normalised, so the dot product is the cosine
//! similarity. Everything is deterministic (no random start), and the work
//! is spread over all cores.

/// Up to this many embeddings, a search compares with every one.
pub const EXACT_MAX: usize = 2000;
/// Rounds of k-means for the cell centres.
const ROUNDS: usize = 8;
/// The centres are trained on at most this many embeddings (evenly picked).
const TRAIN_MAX: usize = 40_000;

/// Cosine similarity of two L2-normalised embeddings. Eight sums side by
/// side, so the compiler can use SIMD without reordering a single sum.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0f32; 8];
    let (ca, cb) = (a.chunks_exact(8), b.chunks_exact(8));
    let tail: f32 = ca.remainder().iter().zip(cb.remainder()).map(|(x, y)| x * y).sum();
    for (x, y) in ca.zip(cb) {
        for i in 0..8 {
            acc[i] += x[i] * y[i];
        }
    }
    acc.iter().sum::<f32>() + tail
}

/// `f(i)` for `i` in `0..n`, on all cores, in order.
pub fn parallel<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism().map_or(1, |t| t.get()).min(n).max(1);
    if threads == 1 {
        return (0..n).map(f).collect();
    }
    let chunk = n.div_ceil(threads);
    let f = &f;
    std::thread::scope(|s| {
        let parts: Vec<_> = (0..n)
            .step_by(chunk)
            .map(|start| s.spawn(move || (start..(start + chunk).min(n)).map(f).collect::<Vec<T>>()))
            .collect();
        parts.into_iter().flat_map(|p| p.join().expect("a search thread panicked")).collect()
    })
}

pub struct Index<'a> {
    dim: usize,
    /// `n × dim` embeddings, one after the other.
    data: &'a [f32],
    /// `cells × dim`
    centres: Vec<f32>,
    cells: Vec<Vec<u32>>,
    /// Cells looked into per search.
    probe: usize,
}

impl<'a> Index<'a> {
    /// An index over `data` (`dim` floats per embedding).
    pub fn build(data: &'a [f32], dim: usize) -> Index<'a> {
        let n = data.len().checked_div(dim).unwrap_or(0);
        if n <= EXACT_MAX {
            return Index { dim, data, centres: Vec::new(), cells: vec![(0..n as u32).collect()], probe: 1 };
        }
        let row = |i: usize| &data[i * dim..(i + 1) * dim];
        let cells = ((n as f64).sqrt().round() as usize).max(2);
        // Start from embeddings spread evenly over the list.
        let mut centres: Vec<f32> = (0..cells).flat_map(|c| row(c * n / cells).to_vec()).collect();
        let step = n.div_ceil(TRAIN_MAX);
        let train: Vec<usize> = (0..n).step_by(step).collect();
        for _ in 0..ROUNDS {
            let nearest = parallel(train.len(), |t| closest(&centres, dim, row(train[t])));
            let mut sums = vec![0f32; cells * dim];
            let mut counts = vec![0usize; cells];
            for (t, &c) in nearest.iter().enumerate() {
                counts[c] += 1;
                for (s, v) in sums[c * dim..(c + 1) * dim].iter_mut().zip(row(train[t])) {
                    *s += v;
                }
            }
            for c in 0..cells {
                let sum = &sums[c * dim..(c + 1) * dim];
                let norm = sum.iter().map(|v| v * v).sum::<f32>().sqrt();
                // An empty cell keeps its centre.
                if counts[c] > 0 && norm > 0.0 {
                    for (d, s) in centres[c * dim..(c + 1) * dim].iter_mut().zip(sum) {
                        *d = s / norm;
                    }
                }
            }
        }
        let nearest = parallel(n, |i| closest(&centres, dim, row(i)));
        let mut lists = vec![Vec::new(); cells];
        for (i, c) in nearest.into_iter().enumerate() {
            lists[c].push(i as u32);
        }
        // An eighth of the cells (at least 12): the neighbours that matter
        // (similar enough to be the same person) nearly always lie in them
        // (99% of those ≥ 0.55 in `tests`, 128 numbers per face like SFace).
        let probe = (cells / 8).max(12).min(cells);
        Index { dim, data, centres, cells: lists, probe }
    }

    pub fn len(&self) -> usize {
        self.data.len().checked_div(self.dim).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn row(&self, i: usize) -> &[f32] {
        &self.data[i * self.dim..(i + 1) * self.dim]
    }

    /// The (at most) `k` embeddings most similar to `query` with a
    /// similarity of at least `min`, most similar first, leaving out
    /// `skip` (the query's own position).
    pub fn search(&self, query: &[f32], k: usize, min: f32, skip: Option<usize>) -> Vec<(usize, f32)> {
        let cells: Vec<usize> = if self.cells.len() == 1 {
            vec![0]
        } else {
            let mut by_centre: Vec<(usize, f32)> = (0..self.cells.len())
                .map(|c| (c, dot(query, &self.centres[c * self.dim..(c + 1) * self.dim])))
                .collect();
            by_centre.select_nth_unstable_by(self.probe - 1, |a, b| b.1.total_cmp(&a.1));
            by_centre[..self.probe].iter().map(|&(c, _)| c).collect()
        };
        let mut found: Vec<(usize, f32)> = Vec::new();
        for c in cells {
            for &i in &self.cells[c] {
                let i = i as usize;
                if Some(i) == skip {
                    continue;
                }
                let sim = dot(query, self.row(i));
                if sim >= min {
                    found.push((i, sim));
                }
            }
        }
        found.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        found.truncate(k);
        found
    }
}

fn closest(centres: &[f32], dim: usize, v: &[f32]) -> usize {
    let mut best = (0, f32::MIN);
    for (c, centre) in centres.chunks_exact(dim).enumerate() {
        let sim = dot(v, centre);
        if sim > best.1 {
            best = (c, sim);
        }
    }
    best.0
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small deterministic random generator (xorshift), for test data.
    pub struct Rng(pub u64);

    impl Rng {
        pub fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
        }

        pub fn unit(&mut self, dim: usize) -> Vec<f32> {
            normalised((0..dim).map(|_| self.next()).collect())
        }
    }

    pub fn normalised(mut v: Vec<f32>) -> Vec<f32> {
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= norm);
        v
    }

    /// `people` random people with `per` faces each: the person's
    /// embedding plus noise, so faces of one person score ~0.5–0.8.
    pub fn faces(people: usize, per: usize, dim: usize, seed: u64) -> Vec<f32> {
        let mut rng = Rng(seed);
        let centres: Vec<Vec<f32>> = (0..people).map(|_| rng.unit(dim)).collect();
        let mut data = Vec::with_capacity(people * per * dim);
        for i in 0..people * per {
            let c = &centres[i % people];
            let noise = rng.unit(dim);
            data.extend(normalised(c.iter().zip(&noise).map(|(a, b)| a + 0.7 * b).collect()));
        }
        data
    }

    #[test]
    fn dot_is_the_plain_sum() {
        let a: Vec<f32> = (0..13).map(|i| i as f32).collect();
        let b: Vec<f32> = (0..13).map(|i| 1.0 - i as f32 / 10.0).collect();
        let plain: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        assert!((dot(&a, &b) - plain).abs() < 1e-4);
        assert_eq!(parallel(10, |i| i * 2), (0..10).map(|i| i * 2).collect::<Vec<_>>());
        assert!(parallel(0, |i| i).is_empty());
    }

    #[test]
    fn finds_nearly_all_close_neighbours() {
        let dim = 128;
        let data = faces(350, 12, dim, 7);
        let index = Index::build(&data, dim);
        assert!(index.cells.len() > 1, "large enough for cells");
        let n = index.len();
        let (mut want, mut got) = (0, 0);
        for i in (0..n).step_by(11) {
            let q = index.row(i);
            let exact: Vec<usize> = (0..n).filter(|&j| j != i && dot(q, index.row(j)) >= 0.55).collect();
            let found = index.search(q, 1000, 0.55, Some(i));
            assert!(found.windows(2).all(|w| w[0].1 >= w[1].1));
            assert!(found.iter().all(|&(j, s)| j != i && s >= 0.55));
            want += exact.len();
            got += exact.iter().filter(|j| found.iter().any(|f| f.0 == **j)).count();
        }
        assert!(want > 0);
        assert!(got as f64 >= 0.97 * want as f64, "recall {got}/{want}");

        // Small sets are searched exactly.
        let small = Index::build(&data[..100 * dim], dim);
        let found = small.search(small.row(3), 5, -1.0, None);
        assert_eq!(found[0].0, 3);
        assert_eq!(found.len(), 5);
    }
}
