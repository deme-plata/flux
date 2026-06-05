// Flux Benchmarks — performance measurements for the Flux Foundation
//
// Run: cargo test --package flux-search -- bench --nocapture

#[cfg(test)]
mod bench {
    use crate::*;

    #[test]
    fn bench_index_1000_docs() {
        let mut engine = SearchEngine::new();
        let start = std::time::Instant::now();

        for i in 0..1000 {
            engine.index_document(Document {
                id: format!("doc-{}", i),
                url: format!("https://example.com/page/{}", i),
                title: format!("Document {} - Rust Performance {}", i, i % 10),
                content: format!("This is test content for document {}. Rust programming language benchmarks. Systems programming safety speed concurrency. {}", i, if i%3==0 {"extra"} else {""}),
                meta_description: Some(format!("Meta for doc {}", i)),
                language: Some("en".into()),
                category: Some(if i%2==0 {"tech"} else {"science"}.into()),
                page_rank: 0.5,
                readability_score: 0.8,
                word_count: 20 + i % 100,
                last_crawled: Some(now_ms() - (i as u64 * 3600_000)),
                content_hash: String::new(),
            });
        }

        let index_ms = start.elapsed().as_millis();
        println!("\n  BENCH: Index 1000 docs: {}ms ({:.1} docs/ms)", index_ms, 1000.0 / index_ms as f64);

        let start = std::time::Instant::now();
        let resp = engine.search(SearchQuery {
            q: "rust programming".into(),
            ..Default::default()
        });
        let query_ms = start.elapsed().as_millis();
        println!("  BENCH: Query 'rust programming' (1000 docs): {}ms, {} results", query_ms, resp.total_results);

        let start = std::time::Instant::now();
        let _ = engine.search(SearchQuery {
            q: "rust programming".into(),
            ..Default::default()
        });
        let cache_ms = start.elapsed().as_millis();
        println!("  BENCH: Cached query: {}ms (cache hit)", cache_ms);
    }

    #[test]
    fn bench_index_10000_docs() {
        let mut engine = SearchEngine::new();
        let start = std::time::Instant::now();

        for i in 0..10000 {
            engine.index_document(Document {
                id: format!("doc-{}", i),
                url: format!("https://example.com/page/{}", i),
                title: format!("Document {} - Topic {}", i, i % 20),
                content: format!("Content for doc {}. Words: rust systems programming safety benchmark test data sample. {}", i, std::iter::repeat("filler ").take((i%50)+1).collect::<String>()),
                meta_description: Some(format!("Meta {}", i)),
                language: Some("en".into()),
                category: Some(if i%3==0 {"tech"} else if i%3==1 {"science"} else {"news"}.into()),
                page_rank: (i as f64 % 10.0) / 10.0,
                readability_score: 0.5 + (i%5) as f64 * 0.1,
                word_count: 10 + i % 200,
                last_crawled: Some(now_ms() - (i as u64 * 60000)),
                content_hash: String::new(),
            });
        }

        let index_ms = start.elapsed().as_millis();
        println!("\n  BENCH: Index 10,000 docs: {}ms ({:.1} docs/ms)", index_ms, 10000.0 / index_ms as f64);

        let start = std::time::Instant::now();
        let resp = engine.search(SearchQuery {
            q: "rust safety".into(),
            ..Default::default()
        });
        let query_ms = start.elapsed().as_millis();
        println!("  BENCH: Query 'rust safety' (10,000 docs): {}ms, {} results", query_ms, resp.total_results);

        let start = std::time::Instant::now();
        let _ = engine.search(SearchQuery {
            q: "rust safety".into(),
            ..Default::default()
        });
        let cache_ms = start.elapsed().as_millis();
        println!("  BENCH: Cached query: {}ms (cache hit)", cache_ms);

        assert!(engine.doc_count() == 10000);
    }

    #[test]
    fn bench_pagerank_100_nodes() {
        let mut calc = PageRankCalculator::new();
        let start = std::time::Instant::now();

        for i in 0..100 {
            calc.add_page(&format!("https://example.com/{}", i));
        }
        for i in 0..100 {
            for j in 0..3 {
                calc.add_link(
                    &format!("https://example.com/{}", i),
                    &format!("https://example.com/{}", (i + j + 1) % 100),
                    1.0,
                );
            }
        }

        let ranks = calc.calculate_pagerank().unwrap();
        let pr_ms = start.elapsed().as_millis();
        println!("\n  BENCH: PageRank 100 nodes, 300 edges: {}ms", pr_ms);
        assert_eq!(ranks.len(), 100);
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}
