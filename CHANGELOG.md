# 変更履歴（Changelog）

Tuck の主な変更点はこのファイルに記録します。
バージョニングは [Semantic Versioning](https://semver.org/lang/ja/) に従います。

## [Unreleased]

## [1.0.0] - 2026-11-20

Tuck の初回正式リリースです。集めた画像・漫画・動画を外部へ出さず、ほかの人に見られない形で手元にまとめて整理・閲覧できる、完全オフラインの暗号化メディア保管庫アプリです。

### 主な機能

集めた画像・漫画・動画やタグ情報はすべて 1 つの暗号化ファイル（`.vault`）に保存します。完全オフラインで動作し、閲覧時にも一時ファイルをディスクに残しません。ショートカットキーや無操作タイマーでいつでも即ロックでき、メモリ上のキャッシュも破棄します。

保管庫の中身は仮想フォルダ・タグ・星評価・メモで整理でき、動画再生や見開き対応の漫画リーダーも備えています。

保管庫を開いている間だけ、使い捨て PIN と QR コードで自宅 LAN や Tailscale 経由のスマホ・別 PC のブラウザから閲覧できます。ロックと同時に切断されます。

取り込みはドラッグ＆ドロップ、クリップボード貼り付け、`yt-dlp` 連携による URL 取り込みに対応しています。暗号化パッケージ（`.tuckpack`）での持ち運びや、通常ファイルでの書き出しもできます。

詳しい仕様やショートカットは [README.md](https://github.com/sorami-wanwan/Tuck/blob/main/README.md) を、暗号化や通信の設計は [SECURITY.md](https://github.com/sorami-wanwan/Tuck/blob/main/SECURITY.md) をご覧ください。

### 対応環境と注意点

対応 OS は macOS（Apple Silicon / Intel）、Windows（x64）、Ubuntu 24.04 / 26.04 LTS（x64）です。macOS（Apple Silicon）で実機検証済みで、Windows と Ubuntu は GitHub Actions のビルドと自動テストで確認しています。

一時ブラウザ公開は通常のローカル LAN では平文 HTTP になるため、自宅など信頼できるネットワーク内だけで利用してください。外出先からアクセスする場合は Tailscale 経由を推奨します。

純粋な Wayland 環境（Ubuntu 等）ではグローバルロックショートカットを利用できませんが、アプリ内ロックと無操作ロックは利用できます。Arc Browser の Cookie 連携は macOS 限定です。

[Unreleased]: https://github.com/sorami-wanwan/Tuck/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/sorami-wanwan/Tuck/releases/tag/v1.0.0
