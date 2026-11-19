## 変更の概要

このプルリクエストで何を変更したか、目的とあわせて書いてください。

## 関連 Issue

関連する Issue 番号があれば記載してください（例: Closes #123）。

## 確認したチェック項目

- [ ] `bun run build` が成功することを確かめた
- [ ] `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` が成功することを確かめた
- [ ] `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` が成功することを確かめた
- [ ] `bun run test:rust` が成功することを確かめた
- [ ] 暗号化、認証、一時ファイル削除などのセキュリティ境界に影響がないことを確かめた
