use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("パスワードが正しくないか、保管庫ファイルが破損しています")]
    WrongPassword,

    #[error("{0}")]
    InvalidPassphrase(String),

    #[error("保管庫が開かれていません")]
    VaultNotOpen,

    #[error("指定された保管庫ファイルが見つかりません: {0}")]
    VaultNotFound(String),

    #[error("指定された場所にすでにファイルが存在します: {0}")]
    VaultAlreadyExists(String),

    #[error("非対応のファイル形式です: {0}")]
    UnsupportedFormat(String),

    #[error("画像データの読み込みに失敗しました: {0}")]
    CorruptedImage(String),

    #[error("クリップボードに有効な画像がありません")]
    ClipboardEmpty,

    #[error("対象のデータが見つかりません: {0}")]
    NotFound(String),

    #[error("入力値が不正です: {0}")]
    InvalidInput(String),

    #[error("{0}")]
    UrlImport(String),

    #[error("{0}")]
    LanShare(String),

    #[error("現在は閲覧専用モードのため変更できません")]
    LanReadOnly,

    #[error("認証が必要です。PINを入力するかQRコードを再スキャンしてください")]
    LanUnauthorized,

    #[error("{0}")]
    LanRateLimited(String),

    #[error("ファイル入出力エラー: {0}")]
    Io(#[from] std::io::Error),

    #[error("データベースエラー: {0}")]
    Db(#[from] rusqlite::Error),
}

impl VaultError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::WrongPassword => "WRONG_PASSWORD",
            Self::InvalidPassphrase(_) => "INVALID_PASSPHRASE",
            Self::VaultNotOpen => "VAULT_NOT_OPEN",
            Self::VaultNotFound(_) => "VAULT_NOT_FOUND",
            Self::VaultAlreadyExists(_) => "VAULT_ALREADY_EXISTS",
            Self::UnsupportedFormat(_) => "UNSUPPORTED_FORMAT",
            Self::CorruptedImage(_) => "CORRUPTED_IMAGE",
            Self::ClipboardEmpty => "CLIPBOARD_EMPTY",
            Self::NotFound(_) => "NOT_FOUND",
            Self::InvalidInput(_) => "INVALID_INPUT",
            Self::UrlImport(_) => "URL_IMPORT_ERROR",
            Self::LanShare(_) => "LAN_SHARE_ERROR",
            Self::LanReadOnly => "LAN_READ_ONLY",
            Self::LanUnauthorized => "LAN_UNAUTHORIZED",
            Self::LanRateLimited(_) => "LAN_RATE_LIMITED",
            Self::Io(_) => "IO_ERROR",
            Self::Db(_) => "DB_ERROR",
        }
    }

    pub fn http_status(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self {
            Self::VaultNotOpen | Self::LanReadOnly | Self::WrongPassword => StatusCode::FORBIDDEN,
            Self::NotFound(_) | Self::VaultNotFound(_) => StatusCode::NOT_FOUND,
            Self::InvalidInput(_)
            | Self::UnsupportedFormat(_)
            | Self::InvalidPassphrase(_)
            | Self::CorruptedImage(_)
            | Self::ClipboardEmpty
            | Self::VaultAlreadyExists(_) => StatusCode::BAD_REQUEST,
            Self::LanUnauthorized => StatusCode::UNAUTHORIZED,
            Self::LanRateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::LanShare(_) | Self::UrlImport(_) | Self::Io(_) | Self::Db(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

#[derive(Serialize)]
struct SerializedError {
    code: &'static str,
    message: String,
}

impl Serialize for VaultError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        SerializedError {
            code: self.code(),
            message: self.to_string(),
        }
        .serialize(serializer)
    }
}
