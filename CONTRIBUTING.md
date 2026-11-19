# Tuck へのコントリビュート方法

バグ報告、機能提案、プルリクエストを歓迎します。作業を始める前に、この手順書と [SECURITY.md](SECURITY.md) を確認してください。

## 開発環境のセットアップ

1. Bun と Rust ツールチェーン（`rustfmt` と `clippy` を含む）を用意します。
2. リポジトリの直下で依存パッケージをインストールします。

```bash
bun install
```

3. 開発モードで起動します。

```bash
bun run tauri:dev
```

## 変更時の確認コマンド

プルリクエストを送る前に、以下のチェックがすべてエラーなしで完了することを確かめてください。

```bash
# フロントエンドの型チェックとビルド
bun run build

# Rust のフォーマット確認
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check

# Rust の静的解析（警告ゼロ）
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings

# Rust のユニットテストと統合テスト
bun run test:rust
```

Docker を使って Ubuntu 環境の Linux ビルドとテスト一式をローカルで確認することもできます。

```bash
# 既定の Ubuntu 26.04 でビルド・fmt・clippy・テストを実行
bun run test:docker

# Ubuntu 24.04 を指定して実行する場合
UBUNTU_VERSION=24.04 bun run test:docker

# Docker 側のキャッシュボリュームを削除する場合
docker compose -f docker/compose.test.yml down -v
```

## 実装と設計の注意点

- 既存の暗号化境界やプライバシー保護（閲覧時の一時ファイル非出力、ロック時のキャッシュ破棄、`Host` と Origin の検証、Cookie 用一時ファイルの `0600` 保護とゼロ上書き削除）を弱めないでください。
- UI の文字列は自然な日本語で統一してください。既存の用語（「パッケージ」「単体画像」「詳細パネル」）に合わせます。
- 保管庫ファイル（`*.vault`）やパッケージファイル（`*.tuckpack` / `*.kvpack`）、ローカルのテスト用データや Cookie ファイルをコミットしないでください。
- 重大なセキュリティ上の不具合に関する情報は公開 Issue やプルリクエストに載せず、 [SECURITY.md](SECURITY.md) の案内どおりに非公開で連絡してください。
