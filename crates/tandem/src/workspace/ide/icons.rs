//! The icons of the IDE popups: one Lucide icon per kind of completion entry.
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

#[cfg(test)]
mod tests {
    use super::completion_icon;
    use gpui_kit::assets::IconName;
    use lsp_types::CompletionItemKind;

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
    fn entries_without_a_known_kind_get_the_code_icon() {
        assert_eq!(completion_icon(None), IconName::Code);
        let unknown: CompletionItemKind = serde_json::from_value(serde_json::json!(26)).unwrap();
        assert_eq!(completion_icon(Some(unknown)), IconName::Code);
    }
}
