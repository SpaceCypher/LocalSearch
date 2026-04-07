import Foundation

// MARK: - Query Filter Types

enum QueryFilter: Equatable {
    case kind(String)
    case path(String)
    case after(String)
    case before(String)
    case sizeGreaterThan(Int64)
    case sizeLessThan(Int64)
    case tag(String)
    case contentPhrase(String)
    case exclude(String)
}

// MARK: - Parsed Query Result

struct ParsedQuery {
    let filters: [QueryFilter]
    let text: String
}

// MARK: - Query Parser

struct QueryParser {
    
    static func parse(_ query: String) -> ParsedQuery {
        var filters: [QueryFilter] = []
        var remainingText = query
        
        // Pattern for kind:ext
        let kindPattern = #"kind:(\w+)"#
        if let match = remainingText.range(of: kindPattern, options: .regularExpression) {
            let token = String(remainingText[match])
            if let colonIndex = token.firstIndex(of: ":") {
                let ext = String(token[token.index(after: colonIndex)...])
                filters.append(.kind(ext))
                remainingText = remainingText.replacingOccurrences(of: token, with: "")
            }
        }
        
        // Pattern for in:path
        let pathPattern = #"in:([\w~/.-]+)"#
        if let match = remainingText.range(of: pathPattern, options: .regularExpression) {
            let token = String(remainingText[match])
            if let colonIndex = token.firstIndex(of: ":") {
                let path = String(token[token.index(after: colonIndex)...])
                filters.append(.path(path))
                remainingText = remainingText.replacingOccurrences(of: token, with: "")
            }
        }
        
        // Pattern for after:date
        let afterPattern = #"after:([\d-]+)"#
        if let match = remainingText.range(of: afterPattern, options: .regularExpression) {
            let token = String(remainingText[match])
            if let colonIndex = token.firstIndex(of: ":") {
                let date = String(token[token.index(after: colonIndex)...])
                filters.append(.after(date))
                remainingText = remainingText.replacingOccurrences(of: token, with: "")
            }
        }
        
        // Pattern for before:date
        let beforePattern = #"before:([\d-]+)"#
        if let match = remainingText.range(of: beforePattern, options: .regularExpression) {
            let token = String(remainingText[match])
            if let colonIndex = token.firstIndex(of: ":") {
                let date = String(token[token.index(after: colonIndex)...])
                filters.append(.before(date))
                remainingText = remainingText.replacingOccurrences(of: token, with: "")
            }
        }
        
        // Pattern for size:>10mb or size:<5gb
        let sizePattern = #"size:([><])(\d+)(mb|gb|kb)"#
        if let match = remainingText.range(of: sizePattern, options: [.regularExpression, .caseInsensitive]) {
            let token = String(remainingText[match])
            let components = token.components(separatedBy: ":")
            if components.count == 2 {
                let sizeSpec = components[1]
                let operator_ = String(sizeSpec.prefix(1))
                let numberPart = sizeSpec.dropFirst()
                
                if let value = Int64(numberPart.prefix(while: { $0.isNumber })) {
                    let unit = String(numberPart.drop(while: { $0.isNumber })).lowercased()
                    var bytes: Int64 = value
                    
                    switch unit {
                    case "kb":
                        bytes *= 1024
                    case "mb":
                        bytes *= 1024 * 1024
                    case "gb":
                        bytes *= 1024 * 1024 * 1024
                    default:
                        break
                    }
                    
                    if operator_ == ">" {
                        filters.append(.sizeGreaterThan(bytes))
                    } else if operator_ == "<" {
                        filters.append(.sizeLessThan(bytes))
                    }
                    
                    remainingText = remainingText.replacingOccurrences(of: token, with: "")
                }
            }
        }
        
        // Pattern for tag:name
        let tagPattern = #"tag:(\w+)"#
        if let match = remainingText.range(of: tagPattern, options: .regularExpression) {
            let token = String(remainingText[match])
            if let colonIndex = token.firstIndex(of: ":") {
                let tag = String(token[token.index(after: colonIndex)...])
                filters.append(.tag(tag))
                remainingText = remainingText.replacingOccurrences(of: token, with: "")
            }
        }
        
        // Pattern for content:"phrase"
        let contentPattern = #"content:"([^"]+)""#
        if let match = remainingText.range(of: contentPattern, options: .regularExpression) {
            let token = String(remainingText[match])
            // Extract text between quotes
            if let startQuote = token.firstIndex(of: "\""),
               let endQuote = token.lastIndex(of: "\""),
               startQuote != endQuote {
                let phrase = String(token[token.index(after: startQuote)..<endQuote])
                filters.append(.contentPhrase(phrase))
                remainingText = remainingText.replacingOccurrences(of: token, with: "")
            }
        }
        
        // Pattern for -word (negation)
        let excludePattern = #"-(\w+)"#
        var searchRange = remainingText.startIndex..<remainingText.endIndex
        while let match = remainingText.range(of: excludePattern, options: .regularExpression, range: searchRange) {
            let token = String(remainingText[match])
            let term = String(token.dropFirst()) // Remove the '-'
            filters.append(.exclude(term))
            remainingText = remainingText.replacingOccurrences(of: token, with: "", options: [], range: match)
            searchRange = match.lowerBound..<remainingText.endIndex
        }
        
        // Clean up remaining text
        let cleanedText = remainingText
            .trimmingCharacters(in: .whitespaces)
            .components(separatedBy: .whitespaces)
            .filter { !$0.isEmpty }
            .joined(separator: " ")
        
        return ParsedQuery(filters: filters, text: cleanedText)
    }
}
