package tree_sitter_saldo_test

import (
	"testing"

	tree_sitter "github.com/tree-sitter/go-tree-sitter"
	tree_sitter_saldo "github.com/sebrollen/saldo/bindings/go"
)

func TestCanLoadGrammar(t *testing.T) {
	language := tree_sitter.NewLanguage(tree_sitter_saldo.Language())
	if language == nil {
		t.Errorf("Error loading Saldo grammar")
	}
}
