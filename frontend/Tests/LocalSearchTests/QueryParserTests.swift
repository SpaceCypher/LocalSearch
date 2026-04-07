import XCTest
@testable import LocalSearch

final class QueryParserTests: XCTestCase {
    
    func test_parse_kindFilter() {
        let result = QueryParser.parse("kind:pdf report")
        XCTAssertEqual(result.filters.count, 1)
        if case .kind(let ext) = result.filters.first {
            XCTAssertEqual(ext, "pdf")
        } else {
            XCTFail("Expected kind filter")
        }
        XCTAssertEqual(result.text, "report")
    }
    
    func test_parse_pathScope() {
        let result = QueryParser.parse("in:~/Projects notes")
        XCTAssertEqual(result.filters.count, 1)
        if case .path(let path) = result.filters.first {
            XCTAssertEqual(path, "~/Projects")
        } else {
            XCTFail("Expected path filter")
        }
        XCTAssertEqual(result.text, "notes")
    }
    
    func test_parse_dateFilter_after() {
        let result = QueryParser.parse("after:2024-01-01 report")
        XCTAssertEqual(result.filters.count, 1)
        if case .after(let date) = result.filters.first {
            XCTAssertEqual(date, "2024-01-01")
        } else {
            XCTFail("Expected after filter")
        }
        XCTAssertEqual(result.text, "report")
    }
    
    func test_parse_negation() {
        let result = QueryParser.parse("report -draft")
        XCTAssertEqual(result.filters.count, 1)
        if case .exclude(let term) = result.filters.first {
            XCTAssertEqual(term, "draft")
        } else {
            XCTFail("Expected exclude filter")
        }
        XCTAssertEqual(result.text, "report")
    }
    
    func test_parse_sizeFilter() {
        let result = QueryParser.parse("size:>10mb video")
        XCTAssertEqual(result.filters.count, 1)
        if case .sizeGreaterThan(let bytes) = result.filters.first {
            XCTAssertEqual(bytes, 10 * 1024 * 1024)
        } else {
            XCTFail("Expected size filter")
        }
        XCTAssertEqual(result.text, "video")
    }
    
    func test_parse_multipleFilters() {
        let result = QueryParser.parse("kind:pdf in:~/Work after:2024-01-01 budget")
        XCTAssertEqual(result.filters.count, 3)
        XCTAssertEqual(result.text, "budget")
    }
    
    func test_parse_contentPhrase() {
        let result = QueryParser.parse("content:\"quarterly revenue\"")
        XCTAssertEqual(result.filters.count, 1)
        if case .contentPhrase(let phrase) = result.filters.first {
            XCTAssertEqual(phrase, "quarterly revenue")
        } else {
            XCTFail("Expected content phrase filter")
        }
        XCTAssertEqual(result.text, "")
    }
    
    func test_parse_noFilters_returnsFullText() {
        let result = QueryParser.parse("hello world")
        XCTAssertTrue(result.filters.isEmpty)
        XCTAssertEqual(result.text, "hello world")
    }
}
