//! Semantic-tokens legend and byte-range → LSP delta/UTF-16 encoding.

use analyzer_core::LineIndex;
use analyzer_ide::{SemToken, TokenMods, TokenType};
use lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};

/// Token-type legend (index order MUST match `token_type_index`).
fn legend_types() -> Vec<SemanticTokenType> {
    vec![
        SemanticTokenType::KEYWORD,            // 0
        SemanticTokenType::TYPE,               // 1
        SemanticTokenType::STRUCT,             // 2
        SemanticTokenType::ENUM,               // 3
        SemanticTokenType::ENUM_MEMBER,        // 4
        SemanticTokenType::TYPE_PARAMETER,     // 5
        SemanticTokenType::PARAMETER,          // 6
        SemanticTokenType::VARIABLE,           // 7
        SemanticTokenType::PROPERTY,           // 8
        SemanticTokenType::FUNCTION,           // 9
        SemanticTokenType::METHOD,             // 10
        SemanticTokenType::NAMESPACE,          // 11
        SemanticTokenType::COMMENT,            // 12
        SemanticTokenType::STRING,             // 13
        SemanticTokenType::NUMBER,             // 14
        SemanticTokenType::OPERATOR,           // 15
        SemanticTokenType::new("punctuation"), // 16 (custom)
    ]
}

/// Modifier legend (bit order MUST match `token_mods_bitset`).
fn legend_modifiers() -> Vec<SemanticTokenModifier> {
    vec![
        SemanticTokenModifier::DECLARATION,     // bit 0
        SemanticTokenModifier::DEFAULT_LIBRARY, // bit 1
    ]
}

pub(crate) fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: legend_types(),
        token_modifiers: legend_modifiers(),
    }
}

fn token_type_index(ty: TokenType) -> u32 {
    match ty {
        TokenType::Keyword => 0,
        TokenType::Type => 1,
        TokenType::Struct => 2,
        TokenType::Enum => 3,
        TokenType::EnumMember => 4,
        TokenType::TypeParameter => 5,
        TokenType::Parameter => 6,
        TokenType::Variable => 7,
        TokenType::Property => 8,
        TokenType::Function => 9,
        TokenType::Method => 10,
        TokenType::Namespace => 11,
        TokenType::Comment => 12,
        TokenType::StringLit => 13,
        TokenType::Number => 14,
        TokenType::Operator => 15,
        TokenType::Punctuation => 16,
    }
}

fn token_mods_bitset(mods: TokenMods) -> u32 {
    let mut bits = 0;
    if mods.declaration {
        bits |= 1 << 0;
    }
    if mods.default_library {
        bits |= 1 << 1;
    }
    bits
}

/// Delta-encode absolute-range tokens into LSP `SemanticToken`s. Tokens that
/// span more than one line (e.g. block comments) are skipped — LSP semantic
/// tokens are single-line.
pub(crate) fn encode_semantic_tokens(tokens: &[SemToken], li: &LineIndex) -> Vec<SemanticToken> {
    let mut out = Vec::new();
    let mut prev_line = 0u32;
    let mut prev_col = 0u32;
    for t in tokens {
        let start = li.line_col(t.range.start());
        let end = li.line_col(t.range.end());
        if end.line != start.line {
            continue;
        }
        let length = end.col.saturating_sub(start.col);
        if length == 0 {
            continue;
        }
        let delta_line = start.line - prev_line;
        let delta_start = if delta_line == 0 {
            start.col - prev_col
        } else {
            start.col
        };
        out.push(SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type: token_type_index(t.ty),
            token_modifiers_bitset: token_mods_bitset(t.mods),
        });
        prev_line = start.line;
        prev_col = start.col;
    }
    out
}
