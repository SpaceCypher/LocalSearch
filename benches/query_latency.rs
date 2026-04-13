use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use localsearch::query::executor::QueryExecutor;
use localsearch::query::parser::Query;
use localsearch::index::delta::{DeltaIndex, Document, DocId, Posting, FIELD_FILENAME, FIELD_PATH};
use localsearch::index::bktree::BkTree;
use localsearch::index::trie::PathTrie;
use localsearch::index::trigram::TrigramIndex;
use std::collections::HashMap;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;

fn build_bench_executor() -> QueryExecutor {
    let mut delta_index = DeltaIndex::new(512 * 1024 * 1024);
    let mut bk_tree = BkTree::new();
    let mut path_trie = PathTrie::new();
    let trigram_index = TrigramIndex::new();
    let phonetic_index = HashMap::new();

    let terms = [
        "quarterly", "budget", "report", "meeting", "invoice", "design", "spec", "alpha", "beta", "gamma",
        "roadmap", "notes", "proposal", "review", "planning", "legal", "contract", "engineering", "ops", "finance",
    ];

    for i in 0..50_000u64 {
        let path = format!("/bench/project_{}/file_{}_{}.txt", i % 400, i, terms[(i as usize) % terms.len()]);
        let doc_id = DocId(i + 1);

        let mut postings = HashMap::new();
        let term_a = terms[(i as usize) % terms.len()].to_string();
        let term_b = terms[((i + 3) as usize) % terms.len()].to_string();

        postings.insert(
            term_a.clone(),
            Posting {
                doc_id,
                term_freq: 2,
                field_mask: FIELD_FILENAME | FIELD_PATH,
                positions: vec![0, 3],
            },
        );
        postings.insert(
            term_b.clone(),
            Posting {
                doc_id,
                term_freq: 1,
                field_mask: FIELD_FILENAME,
                positions: vec![1],
            },
        );

        let _ = delta_index.insert_document(
            Document {
                doc_id,
                path: path.clone(),
                content_hash: i.wrapping_mul(31),
            },
            postings,
        );
        path_trie.insert(&path, doc_id);
        bk_tree.insert(&term_a);
        bk_tree.insert(&term_b);
    }

    path_trie.rebuild_prefix_cache(100);
    
    QueryExecutor::new(delta_index, bk_tree, path_trie, trigram_index, phonetic_index)
}

fn bench_query_latency(c: &mut Criterion) {
    let executor = build_bench_executor();
    let query_terms = [
        "quarterly", "budget", "report", "meeting", "invoice", "design", "spec", "alpha", "beta", "gamma",
        "roadmap", "notes", "proposal", "review", "planning", "legal", "contract", "engineering", "ops", "finance",
    ];
    let mut rng = StdRng::seed_from_u64(42);
    let queries: Vec<Query> = (0..200)
        .map(|_| {
            let a = query_terms[rng.gen_range(0..query_terms.len())];
            let b = query_terms[rng.gen_range(0..query_terms.len())];
            Query::parse(&format!("{} {}", a, b)).unwrap()
        })
        .collect();

    c.bench_with_input(BenchmarkId::new("query_latency", "warm_50k_200q"), &queries, |b, qs| {
        let mut idx = 0usize;
        b.iter(|| {
            let q = qs[idx % qs.len()].clone();
            idx = idx.wrapping_add(1);
            let _ = executor.execute(q, None);
        })
    });
}

criterion_group!(benches, bench_query_latency);
criterion_main!(benches);
