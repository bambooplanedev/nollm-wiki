//! Synthetic corpus generator (`wiki generate`): a seeded, deterministic set of cross-linked source files for benchmarks and tests.

use crate::model::slugify;
use std::path::{Path, PathBuf};

const TOPICS: &[&str] = &[
    "Gradient Descent",
    "Attention Mechanism",
    "Tokenization",
    "Embedding Layer",
    "Transformer Block",
    "Backpropagation",
    "Batch Normalization",
    "Dropout",
    "Learning Rate Schedule",
    "Cross Entropy Loss",
    "Positional Encoding",
    "Layer Normalization",
    "Residual Connection",
    "Self Attention",
    "KV Cache",
    "Beam Search",
    "Greedy Decoding",
    "Top-K Sampling",
    "Temperature Scaling",
    "Fine Tuning",
    "LoRA Adapter",
    "Quantization",
    "Pruning",
    "Distillation",
    "Vector Index",
    "Cosine Similarity",
    "Hybrid Search",
    "Reranking",
];

const TEMPLATES: &[&str] = &[
    "{a} is often paired with {b} in production pipelines.",
    "When debugging {a}, engineers trace the issue back to {b}.",
    "{a} builds directly on the ideas behind {b}.",
    "A common mistake is tuning {a} without first checking {b}.",
];

const FILLER: &[&str] = &[
    "This note was captured during a debugging session and may be incomplete.",
    "Revisit this after the next benchmark run.",
    "Numbers here are approximate and were not re-verified.",
];

/// Deterministic `SplitMix64` — stable across platforms and Rust versions.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

pub fn generate_corpus(
    output_dir: &Path,
    num_files: usize,
    seed: u64,
) -> std::io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(output_dir)?;
    let mut rng = Rng(seed);

    let all_topics: Vec<String> = (0..num_files)
        .map(|i| {
            let base = TOPICS[i % TOPICS.len()];
            let suffix = i / TOPICS.len();
            if suffix == 0 {
                base.to_string()
            } else {
                format!("{base} v{}", suffix + 1)
            }
        })
        .collect();

    let mut written = Vec::new();
    for topic in &all_topics {
        let slug = slugify(topic);
        let mut lines: Vec<String> = Vec::new();

        if rng.next_u64().is_multiple_of(2) {
            lines.push(format!("# {topic}"));
        } else {
            lines.push(topic.to_uppercase());
        }
        if rng.next_u64() % 10 < 7 {
            lines.push(format!(
                "created: 2026-0{}-{}",
                1 + rng.below(6),
                10 + rng.below(18)
            ));
        }
        if rng.next_u64() % 10 < 4 {
            lines.push(format!("aliases: {slug}, {slug}_notes"));
        }
        lines.push(String::new());

        let others: Vec<&String> = all_topics.iter().filter(|t| *t != topic).collect();
        if !others.is_empty() {
            let k = 1 + rng.below(3.min(others.len()));
            for _ in 0..k {
                let other = others[rng.below(others.len())];
                let tmpl = TEMPLATES[rng.below(TEMPLATES.len())];
                lines.push(tmpl.replace("{a}", topic).replace("{b}", other));
            }
        }
        lines.push(String::new());
        lines.push(FILLER[rng.below(FILLER.len())].to_string());

        let content = lines.join("\n") + "\n";
        let path = output_dir.join(format!("{slug}.txt"));
        std::fs::write(&path, content)?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn deterministic_for_same_seed() {
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        let pa = generate_corpus(a.path(), 15, 42).unwrap();
        let pb = generate_corpus(b.path(), 15, 42).unwrap();
        assert_eq!(pa.len(), 15);
        for (fa, fb) in pa.iter().zip(pb.iter()) {
            let ca = std::fs::read_to_string(fa).unwrap();
            let name = fb.file_name().unwrap();
            let cb = std::fs::read_to_string(b.path().join(name)).unwrap();
            assert_eq!(ca, cb);
        }
    }

    #[test]
    fn file_count_matches_request() {
        let d = tempdir().unwrap();
        let paths = generate_corpus(d.path(), 37, 7).unwrap();
        assert_eq!(paths.len(), 37);
    }
}
