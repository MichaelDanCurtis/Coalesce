/// Multilingual keyword sets for the 15-dimension scorer.
/// Covers: English, Chinese, Japanese, Russian, German, Spanish, Portuguese, Korean, Arabic.

pub static CODE_KEYWORDS: &[&str] = &[
    // English
    "function", "class", "def", "return", "import", "const", "let", "var",
    "async", "await", "struct", "impl", "enum", "interface", "public", "private",
    "static", "void", "int", "string", "bool", "for", "while", "if", "else",
    "try", "catch", "throw", "SELECT", "INSERT", "UPDATE", "DELETE", "FROM",
    "WHERE", "JOIN", "CREATE TABLE", "ALTER", "DROP", "INDEX",
    "```", "fn ", "pub fn", "mod ", "use ", "crate::", "self.", "Self::",
    // Chinese
    "函数", "类", "返回", "导入", "变量", "循环", "条件",
    // Japanese
    "関数", "クラス", "変数", "戻り値",
    // Korean
    "함수", "클래스", "변수",
];

pub static REASONING_KEYWORDS: &[&str] = &[
    // English
    "think", "reason", "analyze", "prove", "derive", "deduce", "infer",
    "evaluate", "assess", "consider", "examine", "investigate", "critique",
    "compare and contrast", "weigh", "deliberate", "hypothesize", "theorize",
    "step by step", "chain of thought", "let's think", "think through",
    "reasoning", "logical", "argument", "conclusion", "premise",
    // Chinese
    "思考", "推理", "分析", "证明", "推导", "评估", "逐步",
    // Japanese
    "考える", "推論", "分析", "証明", "評価", "段階的に",
    // Russian
    "думать", "рассуждать", "анализировать", "доказать",
    // German
    "denken", "analysieren", "beweisen", "schlussfolgern",
    // Spanish
    "pensar", "razonar", "analizar", "demostrar",
    // Portuguese
    "pensar", "raciocinar", "analisar", "demonstrar",
    // Korean
    "생각하다", "분석하다", "추론하다",
    // Arabic
    "تحليل", "تفكير", "استنتاج",
];

pub static TECHNICAL_KEYWORDS: &[&str] = &[
    "algorithm", "architecture", "cryptography", "distributed", "consensus",
    "optimization", "complexity", "polynomial", "exponential", "logarithmic",
    "concurrency", "parallelism", "deadlock", "mutex", "semaphore",
    "binary tree", "hash map", "linked list", "graph", "dynamic programming",
    "machine learning", "neural network", "gradient", "backpropagation",
    "kubernetes", "docker", "microservice", "api", "endpoint", "middleware",
    "database", "schema", "migration", "index", "query",
    "encryption", "authentication", "authorization", "OAuth", "JWT",
    // Chinese
    "算法", "架构", "加密", "分布式", "优化", "并发",
    // Japanese
    "アルゴリズム", "アーキテクチャ", "暗号化", "最適化",
];

pub static CREATIVE_KEYWORDS: &[&str] = &[
    "write", "create", "generate", "compose", "craft", "design", "invent",
    "story", "poem", "poetry", "essay", "narrative", "fiction", "creative",
    "imagine", "brainstorm", "ideate", "artistic", "literary",
    // Chinese
    "写作", "创作", "生成", "诗歌", "故事", "想象",
    // Japanese
    "書く", "創作", "生成", "詩", "物語",
];

pub static SIMPLE_KEYWORDS: &[&str] = &[
    "summary", "summarize", "explain", "what is", "what are", "define",
    "translate", "convert", "list", "name", "tell me", "how do you",
    "what does", "simple", "brief", "short", "quick", "basic",
    "yes or no", "true or false",
    // Chinese
    "总结", "解释", "什么是", "翻译", "简单", "列出",
    // Japanese
    "要約", "説明", "とは", "翻訳", "簡単",
    // Spanish
    "resumen", "explicar", "qué es", "traducir",
];

pub static MULTI_STEP_KEYWORDS: &[&str] = &[
    "first", "then", "next", "after that", "finally", "step 1", "step 2",
    "1.", "2.", "3.", "phase", "stage", "process", "workflow", "pipeline",
    "sequence", "procedure", "instructions",
    // Chinese
    "首先", "然后", "接下来", "最后", "步骤",
    // Japanese
    "まず", "次に", "最後に", "ステップ",
];

pub static IMPERATIVE_KEYWORDS: &[&str] = &[
    "build", "implement", "design", "refactor", "optimize", "debug",
    "deploy", "configure", "set up", "install", "migrate", "upgrade",
    "rewrite", "restructure", "fix", "patch", "resolve", "handle",
    // Chinese
    "构建", "实现", "设计", "重构", "优化", "调试", "部署",
    // Japanese
    "構築", "実装", "設計", "リファクタリング", "最適化",
];

pub static CONSTRAINT_KEYWORDS: &[&str] = &[
    "must", "required", "shall", "constraint", "limitation", "restriction",
    "mandatory", "necessary", "prerequisite", "condition", "requirement",
    "ensure", "guarantee", "validate",
    // Chinese
    "必须", "要求", "约束", "限制", "确保",
    // Japanese
    "必須", "要件", "制約", "保証",
];

pub static OUTPUT_FORMAT_KEYWORDS: &[&str] = &[
    "json", "yaml", "xml", "csv", "table", "markdown", "html", "toml",
    "format as", "output as", "return as", "structured",
];

pub static REFERENCE_KEYWORDS: &[&str] = &[
    "cite", "reference", "source", "according to", "based on",
    "documentation", "specification", "standard", "rfc", "paper",
];

pub static NEGATION_KEYWORDS: &[&str] = &[
    "not", "don't", "avoid", "without", "never", "exclude", "except",
    "no ", "isn't", "aren't", "shouldn't", "won't", "cannot",
    // Chinese
    "不要", "避免", "没有", "禁止",
    // Japanese
    "しないで", "避ける", "禁止",
];

pub static DOMAIN_SPECIFIC_KEYWORDS: &[&str] = &[
    // Medical
    "diagnosis", "symptom", "treatment", "clinical", "patient", "pathology",
    "pharmaceutical", "dosage",
    // Legal
    "contract", "liability", "jurisdiction", "statute", "compliance",
    "regulation", "plaintiff", "defendant",
    // Financial
    "portfolio", "dividend", "equity", "derivative", "amortization",
    "balance sheet", "revenue", "margin",
    // Chemistry
    "molecule", "compound", "reaction", "catalyst", "synthesis", "polymer",
];

pub static AGENTIC_KEYWORDS: &[&str] = &[
    "iterate", "refine", "loop", "autonomous", "self-correct",
    "agent", "multi-step", "plan", "execute", "observe", "reflect",
    "tool use", "function call", "chain", "orchestrate", "delegate",
    // Chinese
    "迭代", "自主", "代理", "执行", "规划",
    // Japanese
    "反復", "自律", "エージェント", "実行",
];

/// Check if text contains any keyword from a list (case-insensitive).
/// Returns (match_count, matched_keywords).
pub fn count_matches(text: &str, keywords: &[&str]) -> usize {
    let lower = text.to_lowercase();
    keywords
        .iter()
        .filter(|kw| lower.contains(&kw.to_lowercase()))
        .count()
}

/// Check if text contains code blocks (``` fences)
pub fn has_code_blocks(text: &str) -> bool {
    text.contains("```")
}

/// Estimate token count from text (rough: ~4 chars per token for English)
pub fn estimate_tokens(text: &str) -> usize {
    // Simple heuristic: chars / 4 for English, chars / 2 for CJK-heavy text
    let cjk_count = text
        .chars()
        .filter(|c| {
            let cp = *c as u32;
            (0x4E00..=0x9FFF).contains(&cp)   // CJK Unified
            || (0x3040..=0x309F).contains(&cp) // Hiragana
            || (0x30A0..=0x30FF).contains(&cp) // Katakana
            || (0xAC00..=0xD7AF).contains(&cp) // Korean
        })
        .count();

    let total_chars = text.len();
    if cjk_count > total_chars / 4 {
        // CJK-heavy: ~2 chars per token
        total_chars / 2
    } else {
        // English-heavy: ~4 chars per token
        total_chars / 4
    }
}
