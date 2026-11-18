//! The icons of the IDE views: one Lucide icon per kind of completion entry and per kind
//! of symbol.
use gpui_kit::assets::IconName;
use lsp_types::CompletionItemKind;

/// The icon for a completion entry of `kind`; entries without a kind, or of a kind the
/// protocol does not define, get the generic code icon.
pub(super) fn completion_icon(kind: Option<CompletionItemKind>) -> IconName {
    let Some(kind) = kind else {
        return IconName::Code;
    };
    match kind {
        CompletionItemKind::TEXT => IconName::CaseSensitive,
        CompletionItemKind::METHOD => IconName::Box,
        CompletionItemKind::FUNCTION => IconName::SquareFunction,
        CompletionItemKind::CONSTRUCTOR => IconName::Hammer,
        CompletionItemKind::FIELD => IconName::Tag,
        CompletionItemKind::VARIABLE => IconName::Variable,
        CompletionItemKind::CLASS => IconName::Shapes,
        CompletionItemKind::INTERFACE => IconName::Plug,
        CompletionItemKind::MODULE => IconName::Package,
        CompletionItemKind::PROPERTY => IconName::Wrench,
        CompletionItemKind::UNIT => IconName::Ruler,
        CompletionItemKind::VALUE => IconName::Binary,
        CompletionItemKind::ENUM => IconName::ListOrdered,
        CompletionItemKind::KEYWORD => IconName::Key,
        CompletionItemKind::SNIPPET => IconName::SquareDashedBottomCode,
        CompletionItemKind::COLOR => IconName::Palette,
        CompletionItemKind::FILE => IconName::FileText,
        CompletionItemKind::REFERENCE => IconName::Link,
        CompletionItemKind::FOLDER => IconName::Folder,
        CompletionItemKind::ENUM_MEMBER => IconName::CircleDot,
        CompletionItemKind::CONSTANT => IconName::Sigma,
        CompletionItemKind::STRUCT => IconName::Blocks,
        CompletionItemKind::EVENT => IconName::Zap,
        CompletionItemKind::OPERATOR => IconName::Asterisk,
        CompletionItemKind::TYPE_PARAMETER => IconName::Type,
        _ => IconName::Code,
    }
}

/// The icon for a document or workspace symbol of SymbolKind number `kind`; numbers the
/// protocol does not define, and 0 for a symbol without a kind, get the generic code icon.
pub(super) fn symbol_icon(kind: u32) -> IconName {
    match kind {
        1 => IconName::FileText,        // File
        2 => IconName::Package,         // Module
        3 => IconName::Braces,          // Namespace
        4 => IconName::Package,         // Package
        5 => IconName::Shapes,          // Class
        6 => IconName::Box,             // Method
        7 => IconName::Wrench,          // Property
        8 => IconName::Tag,             // Field
        9 => IconName::Hammer,          // Constructor
        10 => IconName::ListOrdered,    // Enum
        11 => IconName::Plug,           // Interface
        12 => IconName::SquareFunction, // Function
        13 => IconName::Variable,       // Variable
        14 => IconName::Sigma,          // Constant
        15 => IconName::CaseSensitive,  // String
        16 => IconName::Binary,         // Number
        17 => IconName::CircleDot,      // Boolean
        18 => IconName::Brackets,       // Array
        19 => IconName::Braces,         // Object
        20 => IconName::Key,            // Key
        21 => IconName::Dot,            // Null
        22 => IconName::CircleDot,      // EnumMember
        23 => IconName::Blocks,         // Struct
        24 => IconName::Zap,            // Event
        25 => IconName::Asterisk,       // Operator
        26 => IconName::Type,           // TypeParameter
        _ => IconName::Code,
    }
}

#[cfg(test)]
mod tests {
    use super::{completion_icon, symbol_icon};
    use gpui_kit::assets::IconName;
    use lsp_types::{CompletionItemKind, SymbolKind};

    #[test]
    fn every_completion_kind_has_its_icon() {
        let table = [
            (CompletionItemKind::TEXT, IconName::CaseSensitive),
            (CompletionItemKind::METHOD, IconName::Box),
            (CompletionItemKind::FUNCTION, IconName::SquareFunction),
            (CompletionItemKind::CONSTRUCTOR, IconName::Hammer),
            (CompletionItemKind::FIELD, IconName::Tag),
            (CompletionItemKind::VARIABLE, IconName::Variable),
            (CompletionItemKind::CLASS, IconName::Shapes),
            (CompletionItemKind::INTERFACE, IconName::Plug),
            (CompletionItemKind::MODULE, IconName::Package),
            (CompletionItemKind::PROPERTY, IconName::Wrench),
            (CompletionItemKind::UNIT, IconName::Ruler),
            (CompletionItemKind::VALUE, IconName::Binary),
            (CompletionItemKind::ENUM, IconName::ListOrdered),
            (CompletionItemKind::KEYWORD, IconName::Key),
            (
                CompletionItemKind::SNIPPET,
                IconName::SquareDashedBottomCode,
            ),
            (CompletionItemKind::COLOR, IconName::Palette),
            (CompletionItemKind::FILE, IconName::FileText),
            (CompletionItemKind::REFERENCE, IconName::Link),
            (CompletionItemKind::FOLDER, IconName::Folder),
            (CompletionItemKind::ENUM_MEMBER, IconName::CircleDot),
            (CompletionItemKind::CONSTANT, IconName::Sigma),
            (CompletionItemKind::STRUCT, IconName::Blocks),
            (CompletionItemKind::EVENT, IconName::Zap),
            (CompletionItemKind::OPERATOR, IconName::Asterisk),
            (CompletionItemKind::TYPE_PARAMETER, IconName::Type),
        ];
        for (number, (kind, icon)) in (1..).zip(table) {
            let parsed: CompletionItemKind =
                serde_json::from_value(serde_json::json!(number)).unwrap();
            assert!(parsed == kind, "kind {number} is out of order in the table");
            assert_eq!(completion_icon(Some(kind)), icon, "kind {number}");
        }
    }

    #[test]
    fn every_symbol_kind_has_its_icon() {
        let table = [
            (SymbolKind::FILE, IconName::FileText),
            (SymbolKind::MODULE, IconName::Package),
            (SymbolKind::NAMESPACE, IconName::Braces),
            (SymbolKind::PACKAGE, IconName::Package),
            (SymbolKind::CLASS, IconName::Shapes),
            (SymbolKind::METHOD, IconName::Box),
            (SymbolKind::PROPERTY, IconName::Wrench),
            (SymbolKind::FIELD, IconName::Tag),
            (SymbolKind::CONSTRUCTOR, IconName::Hammer),
            (SymbolKind::ENUM, IconName::ListOrdered),
            (SymbolKind::INTERFACE, IconName::Plug),
            (SymbolKind::FUNCTION, IconName::SquareFunction),
            (SymbolKind::VARIABLE, IconName::Variable),
            (SymbolKind::CONSTANT, IconName::Sigma),
            (SymbolKind::STRING, IconName::CaseSensitive),
            (SymbolKind::NUMBER, IconName::Binary),
            (SymbolKind::BOOLEAN, IconName::CircleDot),
            (SymbolKind::ARRAY, IconName::Brackets),
            (SymbolKind::OBJECT, IconName::Braces),
            (SymbolKind::KEY, IconName::Key),
            (SymbolKind::NULL, IconName::Dot),
            (SymbolKind::ENUM_MEMBER, IconName::CircleDot),
            (SymbolKind::STRUCT, IconName::Blocks),
            (SymbolKind::EVENT, IconName::Zap),
            (SymbolKind::OPERATOR, IconName::Asterisk),
            (SymbolKind::TYPE_PARAMETER, IconName::Type),
        ];
        for (number, (kind, icon)) in (1..).zip(table) {
            let parsed: SymbolKind = serde_json::from_value(serde_json::json!(number)).unwrap();
            assert!(parsed == kind, "kind {number} is out of order in the table");
            assert_eq!(symbol_icon(number), icon, "kind {number}");
        }
    }

    #[test]
    fn symbols_without_a_known_kind_get_the_code_icon() {
        assert_eq!(symbol_icon(0), IconName::Code);
        assert_eq!(symbol_icon(27), IconName::Code);
        assert_eq!(symbol_icon(u32::MAX), IconName::Code);
    }

    #[test]
    fn entries_without_a_known_kind_get_the_code_icon() {
        assert_eq!(completion_icon(None), IconName::Code);
        let unknown: CompletionItemKind = serde_json::from_value(serde_json::json!(26)).unwrap();
        assert_eq!(completion_icon(Some(unknown)), IconName::Code);
    }
}
