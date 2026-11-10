#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderKind {
    Fabric,
    Forge,
    NeoForge,
    Unknown,
}

impl LoaderKind {
    pub fn from_args(args: &[String]) -> Self {
        let explicit_mode = args
            .windows(2)
            .find(|pair| pair[0] == "--mode")
            .map(|pair| pair[1].as_str())
            .or_else(|| args.iter().find_map(|arg| arg.strip_prefix("--mode=")));

        let positional_mode =
            args.iter()
                .skip(1)
                .find_map(|arg| match arg.to_ascii_lowercase().as_str() {
                    "fabric" | "forge" | "neoforge" => Some(arg.as_str()),
                    _ => None,
                });

        let mode = explicit_mode.or(positional_mode);

        match mode.map(str::to_ascii_lowercase).as_deref() {
            Some("fabric") => Self::Fabric,
            Some("forge") => Self::Forge,
            Some("neoforge") => Self::NeoForge,
            Some(_) => Self::Unknown,
            None => Self::Fabric,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
            Self::Unknown => "unknown",
        }
    }
}
