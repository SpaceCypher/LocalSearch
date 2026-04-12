use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use localsearch::query::executor::QueryExecutor;
use localsearch::query::parser::Query;
use localsearch::index::delta::DeltaIndex;
use localsearch::index::bktree::BkTree;
use localsearch::index::trie::PathTrie;
use localsearch::index::trigram::TrigramIndex;
use std::collections::HashMap;

fn build_bench_executor() -> QueryExecutor {
    let delta_index = DeltaIndex::new(50 * 1024 * 1024);
    let bk_tree = BkTree::new();
    let path_trie = PathTrie::new();
    let trigram_index = TrigramIndex::new();
    let phonetic_index = HashMap::new();
    
    QueryExecutor::new(delta_index, bk_tree, path_trie, trigram_index, phonetic_index)
}

fn bench_query_latency(c: &mut Criterion) {
    let executor = build_bench_executor();
    let query_str = "test query";
    let query = Query::parse(query_str).unwrap();

    c.bench_with_input(BenchmarkId::new("query_latency", "warm"), &query, |b, q| {
        b.iter(|| {
            let _ = executor.execute(q.clone());
        })
    });
}

criterion_group!(benches, bench_query_latency);
criterion_main!(benches);
