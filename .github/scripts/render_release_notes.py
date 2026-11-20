#!/usr/bin/env python3
"""Render GitHub Release notes from a CHANGELOG.md version section."""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

FOOTER_TEMPLATE = """## 配布ファイル一覧

お使いの OS に合わせてインストーラまたはパッケージをダウンロードしてください。

| OS | 配布ファイル |
| --- | --- |
| macOS (Apple Silicon / M1〜M4) | `Tuck_{version}_aarch64.dmg` |
| macOS (Intel) | `Tuck_{version}_x64.dmg` |
| Windows (x64) | `Tuck_{version}_x64-setup.exe`（推奨 / ユーザー領域へインストール）または `Tuck_{version}_x64_*.msi` |
| Ubuntu 24.04 / 26.04 LTS (x64) | `Tuck_{version}_amd64.deb`（推奨 / `sudo apt install ./Tuck_{version}_amd64.deb`）または `Tuck_{version}_amd64.AppImage` |

### 初回起動時の注意

本リリースの配布バイナリにはコード署名を付与していません。初回起動時に OS の警告が表示される場合は、次の手順で許可してください。

- macOS: アプリを `アプリケーション` フォルダへコピーしたあと、右クリック（Control + クリック）から「開く」を選ぶか、「システム設定 > プライバシーとセキュリティ」から起動を許可してください。起動できない場合はターミナルで `xattr -dr com.apple.quarantine /Applications/Tuck.app` を実行してください。
- Windows: SmartScreen の警告画面が表示された場合は「詳細情報」をクリックし、「実行」を選択してください。
- Linux (Ubuntu): `.deb` を使う場合は `sudo apt install ./Tuck_{version}_amd64.deb` で依存ライブラリごと導入できます。AppImage を使う場合は実行権限を付与して起動してください。
"""


def normalize_version(raw: str) -> str:
    """Strip a single leading 'v' from the version string."""
    trimmed = raw.strip()
    if trimmed.startswith("v"):
        trimmed = trimmed[1:]
    if not trimmed:
        raise ValueError("version must not be empty")
    return trimmed


def extract_version_section(changelog_text: str, version: str) -> tuple[str, str | None]:
    """Extract the changelog body and optional compare URL for a given version."""
    lines = changelog_text.splitlines()
    heading_re = re.compile(rf"^##\s+\[?v?{re.escape(version)}\]?(?:\s*-\s*.*)?$")
    next_heading_re = re.compile(r"^##\s+")
    ref_def_re = re.compile(r"^\[[^\]]+\]:\s+https?://")
    version_ref_re = re.compile(rf"^\[v?{re.escape(version)}\]:\s+(\S+)\s*$")

    found_heading = False
    collecting = False
    collected: list[str] = []
    compare_url: str | None = None

    for line in lines:
        ref_match = version_ref_re.match(line)
        if ref_match and "/compare/" in ref_match.group(1):
            compare_url = ref_match.group(1)

        if not found_heading:
            if heading_re.match(line):
                found_heading = True
                collecting = True
            continue

        if collecting:
            if next_heading_re.match(line) or ref_def_re.match(line):
                collecting = False
                continue
            collected.append(line)

    if not found_heading:
        raise ValueError(f"Version section [{version}] was not found in changelog")

    while collected and not collected[0].strip():
        collected.pop(0)
    while collected and not collected[-1].strip():
        collected.pop()

    body = "\n".join(collected)
    if not body.strip():
        raise ValueError(f"Version section [{version}] is empty in changelog")

    return body, compare_url


def render_release_notes(changelog_text: str, version: str) -> str:
    """Render complete GitHub release notes combining changelog section and download footer."""
    body, compare_url = extract_version_section(changelog_text, version)
    if compare_url:
        body = f"{body}\n\n変更差分: {compare_url}"

    footer = FOOTER_TEMPLATE.format(version=version).rstrip("\n")
    return f"{body}\n\n---\n\n{footer}\n"


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Render GitHub release notes from CHANGELOG.md."
    )
    parser.add_argument(
        "--changelog",
        default="CHANGELOG.md",
        help="Path to CHANGELOG.md (default: CHANGELOG.md)",
    )
    parser.add_argument(
        "--version",
        required=True,
        help="Release version (e.g. 1.0.0 or v1.0.0)",
    )
    parser.add_argument(
        "--tag",
        default=None,
        help="Git tag name (default: v<version>)",
    )
    parser.add_argument(
        "--repo",
        default=os.environ.get("GITHUB_REPOSITORY", "sorami-wanwan/Tuck"),
        help="GitHub repository in owner/repo format",
    )
    parser.add_argument(
        "--output",
        default="-",
        help="Output file path, or '-' for stdout (default: -)",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv if argv is not None else sys.argv[1:])
    try:
        version = normalize_version(args.version)
        _tag = args.tag or f"v{version}"
        _repo = args.repo
        changelog_path = Path(args.changelog)
        if not changelog_path.is_file():
            raise FileNotFoundError(f"Changelog file not found: {changelog_path}")
        changelog_text = changelog_path.read_text(encoding="utf-8")
        rendered = render_release_notes(changelog_text, version)
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    if args.output == "-":
        sys.stdout.write(rendered)
    else:
        out_path = Path(args.output)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(rendered, encoding="utf-8")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
