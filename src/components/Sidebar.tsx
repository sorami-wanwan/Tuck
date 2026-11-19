import { useEffect, useMemo, useState, type KeyboardEvent } from "react";
import {
  Archive,
  BookPlus,
  ChevronDown,
  ChevronRight,
  ClipboardCheck,
  Download,
  Filter,
  Folder,
  FolderOpen,
  FolderPlus,
  Hash,
  Inbox,
  Layers,
  Link2,
  Lock,
  LogOut,
  Plus,
  Radio,
  Settings,
  ShieldCheck,
  Trash2,
  X,
} from "lucide-react";
import type { DropTarget } from "../hooks/useInternalDrag";
import type {
  FolderNode,
  LanShareStatus,
  TagWithCount,
  VaultStatus,
  ViewSelection,
} from "../types/vault";
import { formatBytes } from "../utils/format";
import { formatShortcutLabel } from "../utils/formatShortcut";
import { ContextMenu, type ContextMenuItem } from "./ContextMenu";

interface SidebarProps {
  status: VaultStatus;
  folders: FolderNode[];
  tags: TagWithCount[];
  view: ViewSelection;
  sidebarFilter: string;
  hoverDropTarget: DropTarget;
  isWebClient?: boolean;
  canEdit?: boolean;
  lanShareStatus?: LanShareStatus;
  isDrawerMode?: boolean;
  onCloseDrawer?: () => void;
  onOpenLanShare?: () => void;
  onLogoutLan?: () => void;
  onSidebarFilterChange: (value: string) => void;
  onSelectView: (next: ViewSelection) => void;
  onToggleTag: (tagName: string) => void;
  onImportFiles: () => void;
  onOpenUrlImport: () => void;
  onImportFolderAsGroup: () => void;
  onToggleClipboardWatch: () => void;
  onOpenMaintenance: () => void;
  onExportFolder: (folderId: string) => void;
  onPlainExportFolder: (folderId: string) => void;
  onOpenLockSettings: () => void;
  onLockNow: () => void;
  onCreateFolder: (parentId: string | null, name: string, color: string) => Promise<void>;
  onRenameFolder: (folderId: string, name: string, color?: string | null) => Promise<void>;
  onMoveFolder: (folderId: string, newParentId: string | null) => Promise<void>;
  onDeleteFolder: (folderId: string) => Promise<void>;
  onBeginFolderDrag: (folderId: string, label: string, x: number, y: number) => void;
}

const FOLDER_COLORS = [
  { value: "", label: "デフォルト" },
  { value: "#4c8dff", label: "ブルー" },
  { value: "#34d399", label: "グリーン" },
  { value: "#f5b942", label: "イエロー" },
  { value: "#f472b6", label: "ピンク" },
  { value: "#a78bfa", label: "パープル" },
];

function formatLanRemainingShort(expiresAtMs: number | null, now: number): string {
  if (expiresAtMs === null) return "無制限";
  const diffSec = Math.max(0, Math.floor((expiresAtMs - now) / 1000));
  const mins = Math.ceil(diffSec / 60);
  if (mins >= 60) {
    const h = Math.floor(mins / 60);
    const m = mins % 60;
    return m > 0 ? `${h}時間${m}分` : `${h}時間`;
  }
  return `${mins}分`;
}

export function Sidebar({
  status,
  folders,
  tags,
  view,
  sidebarFilter,
  hoverDropTarget,
  isWebClient = false,
  canEdit = true,
  lanShareStatus,
  isDrawerMode = false,
  onCloseDrawer,
  onOpenLanShare,
  onLogoutLan,
  onSidebarFilterChange,
  onSelectView,
  onToggleTag,
  onImportFiles,
  onOpenUrlImport,
  onImportFolderAsGroup,
  onToggleClipboardWatch,
  onOpenMaintenance,
  onExportFolder,
  onPlainExportFolder,
  onOpenLockSettings,
  onLockNow,
  onCreateFolder,
  onRenameFolder,
  onMoveFolder,
  onDeleteFolder,
  onBeginFolderDrag,
}: SidebarProps) {
  const [importMenuPos, setImportMenuPos] = useState<{ x: number; y: number } | null>(null);
  const [expandedIds, setExpandedIds] = useState<Set<string>>(() => new Set());
  const [creatingParentId, setCreatingParentId] = useState<string | null | undefined>(undefined);
  const [newFolderName, setNewFolderName] = useState("");
  const [editingFolderId, setEditingFolderId] = useState<string | null>(null);
  const [editFolderName, setEditFolderName] = useState("");
  const [folderMenu, setFolderMenu] = useState<{
    folder: FolderNode;
    x: number;
    y: number;
  } | null>(null);
  const [nowMs, setNowMs] = useState(() => Date.now());

  useEffect(() => {
    if (!lanShareStatus?.running) return;
    setNowMs(Date.now());
    const id = window.setInterval(() => setNowMs(Date.now()), 15_000);
    return () => window.clearInterval(id);
  }, [lanShareStatus?.running]);

  const activeTagSet = useMemo(() => {
    if (view.kind !== "tags") return new Set<string>();
    return new Set(view.tags.map((t) => t.toLowerCase()));
  }, [view]);

  const filterQ = sidebarFilter.trim().toLowerCase();

  const childrenMap = useMemo(() => {
    const map = new Map<string | null, FolderNode[]>();
    for (const f of folders) {
      if (filterQ && !f.name.toLowerCase().includes(filterQ)) continue;
      const key = filterQ ? null : f.parentId;
      const list = map.get(key) ?? [];
      list.push(f);
      map.set(key, list);
    }
    return map;
  }, [folders, filterQ]);

  const filteredTags = useMemo(() => {
    if (!filterQ) return tags;
    return tags.filter((t) => t.name.toLowerCase().includes(filterQ));
  }, [tags, filterQ]);

  const toggleExpand = (folderId: string) => {
    setExpandedIds((prev) => {
      const next = new Set(prev);
      if (next.has(folderId)) next.delete(folderId);
      else next.add(folderId);
      return next;
    });
  };

  const commitCreateFolder = async () => {
    const parentId = creatingParentId === undefined ? null : creatingParentId;
    const trimmed = newFolderName.trim();
    setCreatingParentId(undefined);
    setNewFolderName("");
    if (!trimmed) return;
    if (parentId) {
      setExpandedIds((prev) => new Set(prev).add(parentId));
    }
    await onCreateFolder(parentId, trimmed, "");
  };

  const commitRenameFolder = async (folder: FolderNode) => {
    const trimmed = editFolderName.trim();
    setEditingFolderId(null);
    if (!trimmed || trimmed === folder.name) return;
    await onRenameFolder(folder.id, trimmed, folder.color);
  };

  const renderFolderInput = (depth: number) => (
    <div
      className="folder-tree-row"
      style={{ paddingInlineStart: 8 + depth * 14 }}
    >
      <Folder size={14} style={{ color: "var(--accent-blue)", flexShrink: 0 }} />
      <input
        type="text"
        className="folder-inline-input"
        placeholder="フォルダ名..."
        value={newFolderName}
        autoFocus
        onChange={(e) => setNewFolderName(e.target.value)}
        onBlur={() => void commitCreateFolder()}
        onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
          if (e.key === "Enter") {
            e.preventDefault();
            void commitCreateFolder();
          } else if (e.key === "Escape") {
            setCreatingParentId(undefined);
            setNewFolderName("");
          }
        }}
      />
    </div>
  );

  const renderFolderTree = (parentId: string | null, depth: number) => {
    const list = childrenMap.get(parentId) ?? [];
    return (
      <>
        {list.map((folder) => {
          const children = filterQ ? [] : childrenMap.get(folder.id) ?? [];
          const hasChildren = children.length > 0;
          const isExpanded = expandedIds.has(folder.id);
          const isActive = view.kind === "folder" && view.folderId === folder.id;
          const isDropTarget =
            hoverDropTarget?.type === "folder" && hoverDropTarget.folderId === folder.id;
          const isEditing = editingFolderId === folder.id;

          return (
            <div key={folder.id}>
              <div
                data-drop-folder-id={folder.id}
                className={`folder-tree-row ${isActive ? "active" : ""} ${
                  isDropTarget ? "drop-target-active" : ""
                }`}
                style={{ paddingInlineStart: 6 + depth * 14 }}
                onClick={() => onSelectView({ kind: "folder", folderId: folder.id })}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setFolderMenu({ folder, x: e.clientX, y: e.clientY });
                }}
                onPointerDown={(e) => {
                  if (!canEdit || e.pointerType === "touch" || e.button !== 0 || isEditing) return;
                  if ((e.target as HTMLElement).closest("button, input")) return;
                  onBeginFolderDrag(folder.id, folder.name, e.clientX, e.clientY);
                }}
              >
                <button
                  type="button"
                  className="folder-chevron-btn"
                  style={{ visibility: hasChildren ? "visible" : "hidden" }}
                  aria-label={isExpanded ? "折りたたむ" : "展開する"}
                  onClick={(e) => {
                    e.stopPropagation();
                    toggleExpand(folder.id);
                  }}
                >
                  {isExpanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
                </button>

                {isActive ? (
                  <FolderOpen
                    size={14}
                    style={{ color: folder.color || "var(--accent-blue)", flexShrink: 0 }}
                  />
                ) : (
                  <Folder
                    size={14}
                    style={{ color: folder.color || "var(--text-secondary)", flexShrink: 0 }}
                  />
                )}

                {isEditing ? (
                  <input
                    type="text"
                    className="folder-inline-input"
                    value={editFolderName}
                    autoFocus
                    onClick={(e) => e.stopPropagation()}
                    onChange={(e) => setEditFolderName(e.target.value)}
                    onBlur={() => void commitRenameFolder(folder)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        void commitRenameFolder(folder);
                      } else if (e.key === "Escape") {
                        setEditingFolderId(null);
                      }
                    }}
                  />
                ) : (
                  <>
                    <span className="nav-item-label">{folder.name}</span>
                    <span className="nav-item-count tabular-nums">{folder.itemCount}</span>
                  </>
                )}
              </div>

              {creatingParentId === folder.id && renderFolderInput(depth + 1)}
              {isExpanded && hasChildren && renderFolderTree(folder.id, depth + 1)}
            </div>
          );
        })}
      </>
    );
  };

  const folderContextItems: ContextMenuItem[] = folderMenu
    ? [
        ...(canEdit
          ? [
              {
                id: "sub",
                label: "子フォルダを作成",
                icon: <FolderPlus size={13} />,
                onClick: () => {
                  setExpandedIds((prev) => new Set(prev).add(folderMenu.folder.id));
                  setCreatingParentId(folderMenu.folder.id);
                  setNewFolderName("");
                },
              },
              {
                id: "rename",
                label: "名前を変更",
                onClick: () => {
                  setEditingFolderId(folderMenu.folder.id);
                  setEditFolderName(folderMenu.folder.name);
                },
              },
              {
                id: "color",
                label: "カラーを変更",
                children: FOLDER_COLORS.map((c, idx) => ({
                  id: `color-${idx}`,
                  label: c.label,
                  icon: (
                    <span
                      style={{
                        inlineSize: 10,
                        blockSize: 10,
                        borderRadius: "50%",
                        background: c.value || "#98a2b3",
                        display: "inline-block",
                      }}
                    />
                  ),
                  onClick: () => {
                    void onRenameFolder(folderMenu.folder.id, folderMenu.folder.name, c.value);
                  },
                })),
              },
              ...(folderMenu.folder.parentId
                ? [
                    {
                      id: "move-root",
                      label: "最上位（ルート）へ移動",
                      onClick: () => void onMoveFolder(folderMenu.folder.id, null),
                    },
                  ]
                : []),
            ]
          : []),
        {
          id: "export-plain",
          label: isWebClient
            ? "このフォルダを ZIP でダウンロード"
            : "このフォルダをファイルとして書き出し...",
          icon: <Download size={13} />,
          separatorBefore: canEdit,
          onClick: () => onPlainExportFolder(folderMenu.folder.id),
        },
        ...(canEdit && !isWebClient
          ? [
              {
                id: "export-pkg",
                label: "このフォルダをパッケージに書き出し...",
                icon: <Archive size={13} />,
                onClick: () => onExportFolder(folderMenu.folder.id),
              },
            ]
          : []),
        ...(canEdit
          ? [
              {
                id: "delete",
                label: "フォルダを削除（中のデータは残る）",
                icon: <Trash2 size={13} />,
                danger: true,
                separatorBefore: true,
                onClick: () => void onDeleteFolder(folderMenu.folder.id),
              },
            ]
          : []),
      ]
    : [];

  const importMenuItems: ContextMenuItem[] = [
    {
      id: "import-files",
      label: "画像・動画ファイルを追加...",
      icon: <Plus size={14} />,
      onClick: onImportFiles,
    },
    {
      id: "import-url",
      label: "URLから動画を取り込む...",
      icon: <Link2 size={14} />,
      onClick: onOpenUrlImport,
    },
    ...(!isWebClient
      ? [
          {
            id: "import-manga-folder",
            label: "フォルダを1冊の漫画として取り込む...",
            icon: <BookPlus size={14} />,
            onClick: onImportFolderAsGroup,
          },
        ]
      : []),
  ];

  return (
    <aside className="sidebar" aria-label="ライブラリナビゲーション">
      <div className="sidebar-brand">
        <div className="brand-identity" title="Tuck">
          <span className="brand-badge" aria-label="Tuck">
            <ShieldCheck size={14} />
          </span>
          {isWebClient && (
            <span className={`lan-mode-badge ${canEdit ? "rw" : "ro"}`}>
              {canEdit ? "編集可" : "閲覧のみ"}
            </span>
          )}
        </div>

        <div className="sidebar-actions">
          {!isWebClient && (
            <button
              type="button"
              className={`icon-btn watch-btn ${status.clipboardWatchEnabled ? "watching" : ""}`}
              title={
                status.clipboardWatchEnabled
                  ? "クリップボード自動取り込み：ON（クリックで停止）"
                  : "クリップボード自動取り込み：OFF（クリックで監視開始）"
              }
              aria-label="クリップボード自動取り込みを切替"
              aria-pressed={status.clipboardWatchEnabled}
              onClick={onToggleClipboardWatch}
            >
              <ClipboardCheck size={14} />
            </button>
          )}

          {canEdit && (
            <button
              type="button"
              className={`icon-btn ${importMenuPos ? "active" : ""}`}
              title="画像・動画・漫画を取り込む"
              aria-label="画像・動画・漫画を取り込む"
              onPointerDown={(e) => e.stopPropagation()}
              onClick={(e) => {
                if (importMenuPos) {
                  setImportMenuPos(null);
                  return;
                }
                const rect = e.currentTarget.getBoundingClientRect();
                setImportMenuPos({ x: rect.left, y: rect.bottom + 6 });
              }}
            >
              <Plus size={15} />
            </button>
          )}

          {!isWebClient && onOpenLanShare && (
            <button
              type="button"
              className={`icon-btn lan-share-btn ${lanShareStatus?.running ? "active running" : ""}`}
              title={
                lanShareStatus?.running
                  ? "LAN 一時ブラウザ公開中（クリックで管理）"
                  : "LAN 内の他端末に一時公開..."
              }
              aria-label="LAN 一時ブラウザ公開"
              aria-pressed={Boolean(lanShareStatus?.running)}
              onClick={onOpenLanShare}
            >
              <Radio size={14} />
            </button>
          )}

          {!isWebClient && (
            <button
              type="button"
              className="icon-btn"
              title="保管庫の管理・パッケージ"
              aria-label="保管庫の管理・パッケージ"
              onClick={onOpenMaintenance}
            >
              <Archive size={14} />
            </button>
          )}

          {!isWebClient && (
            <button
              type="button"
              className="icon-btn"
              title="設定"
              aria-label="設定"
              onClick={onOpenLockSettings}
            >
              <Settings size={14} />
            </button>
          )}

          {isWebClient ? (
            <button
              type="button"
              className="icon-btn"
              title="LAN 接続を終了して切断"
              aria-label="LAN 接続を切断"
              onClick={onLogoutLan}
            >
              <LogOut size={14} />
            </button>
          ) : (
            <button
              type="button"
              className="icon-btn"
              title={`即時ロック (${formatShortcutLabel(status.lockSettings.shortcut)})`}
              aria-label="即時ロック"
              onClick={onLockNow}
            >
              <Lock size={14} />
            </button>
          )}

          {isDrawerMode && onCloseDrawer && (
            <button
              type="button"
              className="icon-btn"
              aria-label="メニューを閉じる"
              onClick={onCloseDrawer}
            >
              <X size={15} />
            </button>
          )}
        </div>
      </div>

      <div className="sidebar-scroll">
        <nav className="sidebar-section" aria-label="スマートライブラリ">
          <button
            type="button"
            className={`nav-item ${view.kind === "all" ? "active" : ""}`}
            onClick={() => onSelectView({ kind: "all" })}
          >
            <Inbox size={15} className="nav-item-icon" />
            <span className="nav-item-label">すべて</span>
            <span className="nav-item-count tabular-nums">{status.itemCount}</span>
          </button>

          <button
            type="button"
            className={`nav-item ${view.kind === "uncategorized" ? "active" : ""}`}
            onClick={() => onSelectView({ kind: "uncategorized" })}
          >
            <Folder size={15} className="nav-item-icon" />
            <span className="nav-item-label">未分類</span>
            <span className="nav-item-count tabular-nums">{status.uncategorizedCount}</span>
          </button>

          <button
            type="button"
            className={`nav-item ${view.kind === "untagged" ? "active" : ""}`}
            onClick={() => onSelectView({ kind: "untagged" })}
          >
            <Layers size={15} className="nav-item-icon" />
            <span className="nav-item-label">タグなし</span>
            <span className="nav-item-count tabular-nums">{status.untaggedCount}</span>
          </button>

          <button
            type="button"
            data-drop-trash="true"
            className={`nav-item ${view.kind === "trash" ? "active" : ""} ${
              hoverDropTarget?.type === "trash" ? "drop-target-active" : ""
            }`}
            onClick={() => onSelectView({ kind: "trash" })}
          >
            <Trash2 size={15} className="nav-item-icon" />
            <span className="nav-item-label">ゴミ箱</span>
            <span className="nav-item-count tabular-nums">{status.trashCount}</span>
          </button>
        </nav>

        <div className="sidebar-divider" />

        <section className="sidebar-section" aria-label="仮想フォルダ">
          <div
            data-drop-folder-root="true"
            className={`sidebar-section-header ${
              hoverDropTarget?.type === "folderRoot" ? "drop-target-active" : ""
            }`}
            style={{ borderRadius: 6 }}
          >
            <span>フォルダ ({folders.length})</span>
            {canEdit && (
              <button
                type="button"
                className="icon-btn"
                style={{ inlineSize: 20, blockSize: 20 }}
                title="新しいフォルダを作成"
                aria-label="新しいフォルダを作成"
                onClick={() => {
                  setCreatingParentId(null);
                  setNewFolderName("");
                }}
              >
                <FolderPlus size={12} />
              </button>
            )}
          </div>

          {creatingParentId === null && renderFolderInput(0)}
          {folders.length === 0 && creatingParentId === undefined ? (
            <div style={{ padding: "6px 10px", color: "var(--text-muted)", fontSize: 12 }}>
              フォルダがありません
            </div>
          ) : (
            renderFolderTree(null, 0)
          )}
        </section>

        <div className="sidebar-divider" />

        <section className="sidebar-section" aria-label="タグ一覧">
          <div className="sidebar-section-header">
            <span>タグ ({tags.length})</span>
            {view.kind === "tags" && view.tags.length > 0 && (
              <button
                type="button"
                className="icon-btn"
                style={{ inlineSize: 20, blockSize: 20 }}
                title="タグ選択を解除"
                aria-label="タグ選択を解除"
                onClick={() => onSelectView({ kind: "all" })}
              >
                <X size={12} />
              </button>
            )}
          </div>

          {filteredTags.length === 0 ? (
            <div style={{ padding: "6px 10px", color: "var(--text-muted)", fontSize: 12 }}>
              {tags.length === 0 ? "まだタグがありません" : "一致するタグがありません"}
            </div>
          ) : (
            filteredTags.map((tag) => {
              const isSelected = activeTagSet.has(tag.name.toLowerCase());
              return (
                <button
                  key={tag.id}
                  type="button"
                  className={`nav-item ${isSelected ? "active" : ""}`}
                  title={`${tag.name} (${tag.count}件) — クリックで絞り込み（複数選択可）`}
                  onClick={() => onToggleTag(tag.name)}
                >
                  <Hash size={14} className="nav-item-icon" />
                  <span className="nav-item-label">{tag.name}</span>
                  <span className="nav-item-count tabular-nums">{tag.count}</span>
                </button>
              );
            })
          )}
        </section>
      </div>

      <div className="sidebar-footer">
        <div className="sidebar-filter-box">
          <Filter size={13} style={{ color: "var(--text-muted)", flexShrink: 0 }} />
          <input
            type="text"
            aria-label="フォルダやタグを絞り込み"
            placeholder="フィルター"
            value={sidebarFilter}
            onChange={(e) => onSidebarFilterChange(e.target.value)}
          />
          {sidebarFilter && (
            <button
              type="button"
              className="tag-chip-remove"
              aria-label="フィルターをクリア"
              onClick={() => onSidebarFilterChange("")}
            >
              <X size={12} />
            </button>
          )}
        </div>
        {!isWebClient && lanShareStatus?.running && onOpenLanShare && (
          <button
            type="button"
            className="lan-status-bar"
            onClick={onOpenLanShare}
            title="LAN 公開の管理を開く"
          >
            <span className="lan-status-dot" aria-hidden="true" />
            <span className="lan-status-bar-text">
              LAN 公開中（{lanShareStatus.mode === "readOnly" ? "閲覧のみ" : "編集可"} ・{" "}
              {lanShareStatus.clients.length}台）
            </span>
            <span className="tabular-nums" style={{ color: "var(--text-muted)", fontSize: 10 }}>
              {formatLanRemainingShort(lanShareStatus.expiresAtMs, nowMs)}
            </span>
          </button>
        )}
        <div className="sidebar-stats tabular-nums">
          <span>
            {isWebClient
              ? canEdit
                ? "LAN 接続中（編集可）"
                : "LAN 接続中（閲覧専用）"
              : status.clipboardWatchEnabled
                ? "クリップボード監視中"
                : "暗号化保存"}
          </span>
          <span>{formatBytes(status.totalBytes)}</span>
        </div>
      </div>

      <ContextMenu
        open={importMenuPos !== null}
        x={importMenuPos?.x ?? 0}
        y={importMenuPos?.y ?? 0}
        items={importMenuItems}
        onClose={() => setImportMenuPos(null)}
      />

      <ContextMenu
        open={folderMenu !== null}
        x={folderMenu?.x ?? 0}
        y={folderMenu?.y ?? 0}
        items={folderContextItems}
        onClose={() => setFolderMenu(null)}
      />
    </aside>
  );
}
