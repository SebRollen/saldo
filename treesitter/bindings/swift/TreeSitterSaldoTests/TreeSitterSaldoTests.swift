import XCTest
import SwiftTreeSitter
import TreeSitterSaldo

final class TreeSitterSaldoTests: XCTestCase {
    func testCanLoadGrammar() throws {
        let parser = Parser()
        let language = Language(language: tree_sitter_saldo())
        XCTAssertNoThrow(try parser.setLanguage(language),
                         "Error loading Saldo grammar")
    }
}
