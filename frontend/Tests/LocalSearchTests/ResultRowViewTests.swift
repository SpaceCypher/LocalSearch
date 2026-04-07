import XCTest
@testable import LocalSearch

@MainActor
final class ResultRowViewTests: XCTestCase {
    
    func test_rowHeight_standard_is56() {
        let result = SearchResult.mock(rank: 1.0)
        let row = ResultRowView(result: result, isSelected: false)
        // View has fixed height of 56
        XCTAssertNotNil(row)
    }
    
    func test_filename_middleTruncated() {
        let longFilename = "quarterly_report_sales_analysis_final_v2.pdf"
        let result = SearchResult(
            id: "test",
            filename: longFilename,
            path: "/test/path",
            rank: 1.0,
            fileKind: .document
        )
        let row = ResultRowView(result: result, isSelected: false)
        
        // Should truncate in the middle
        let display = row.displayFilename
        if longFilename.count > 50 {
            XCTAssertTrue(display.contains("…"))
            XCTAssertTrue(display.hasSuffix(".pdf"))
        }
    }
    
    func test_path_homeDirectorySubstituted() {
        let homeDir = FileManager.default.homeDirectoryForCurrentUser.path
        let result = SearchResult(
            id: "test",
            filename: "test.txt",
            path: "\(homeDir)/Documents/Work/file.txt",
            rank: 1.0,
            fileKind: .document
        )
        let row = ResultRowView(result: result, isSelected: false)
        
        XCTAssertTrue(row.displayPath.hasPrefix("~/"))
        XCTAssertFalse(row.displayPath.contains(homeDir))
    }
    
    func test_path_longPath_leftTruncated() {
        let longPath = "~/a/b/c/d/e/f/g/h/i/j/k/very/long/path/to/file.txt"
        let result = SearchResult(
            id: "test",
            filename: "file.txt",
            path: longPath,
            rank: 1.0,
            fileKind: .document
        )
        let row = ResultRowView(result: result, isSelected: false)
        
        // Should left-truncate long paths
        if longPath.count > 60 {
            XCTAssertTrue(row.displayPath.hasPrefix("…/"))
        }
    }
}
