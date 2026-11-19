import {
  CheckSquare,
  ChevronLeft,
  ChevronRight,
  Download,
  LayoutGrid,
  Menu,
  PanelRight,
  Search,
  SlidersHorizontal,
  Square,
  X,
} from "lucide-react";
import type { FolderNode, SortBy, ThumbFitMode, ViewSelection } from "../types/vault";

interface ToolbarProps {
  view: ViewSelection;
  folders: FolderNode[];
  itemCount: number;
  canGoBack: boolean;
  canGoForward: boolean;
  onGoBack: () => void;
  onGoForward: () => void;
  thumbSize: number;
  onThumbSizeChange: (size: number) => void;
  fitMode: ThumbFitMode;
  onToggleFitMode: () => void;
  sortBy: SortBy;
  onSortByChange: (sortBy: SortBy) => void;
  searchText: string;
  onSearchTextChange: (text: string) => void;
  filterOpen: boolean;
  onToggleFilterOpen: () => void;
  minRating: number | null;
  onMinRatingChange: (rating: number | null) => void;
  mimeTypes: string[];
  onToggleMimeType: (mime: string) => void;
  onRemoveTag: (tagName: string) => void;
  onClearAllFilters: () => void;
  inspectorOpen: boolean;
  onToggleInspector: () => void;
  showMenuButton?: boolean;
  mobileDrawerOpen?: boolean;
  onToggleMobileDrawer?: () => void;
  isTouchDevice?: boolean;
  isWebClient?: boolean;
  selectionMode?: boolean;
  onToggleSelectionMode?: () => void;
  onPlainExportFolder?: (folderId: string) => void;
}

const MIME_FILTERS: Array<{ label: string; mime: string }> = [
  { label: "PNG", mime: "image/png" },
  { label: "JPEG", mime: "image/jpeg" },
  { label: "WebP", mime: "image/webp" },
  { label: "GIF", mime: "image/gif" },
  { label: "AVIF", mime: "image/avif" },
  { label: "MP4", mime: "video/mp4" },
  { label: "WebM", mime: "video/webm" },
  { label: "MOV", mime: "video/quicktime" },
];

function viewTitle(view: ViewSelection, folders: FolderNode[]): string {
  switch (view.kind) {
    case "all":
      return "すべて";
    case "uncategorized":
      return "未分類";
    case "untagged":
      return "タグなし";
    case "trash":
      return "ゴミ箱";
    case "folder":
      return folders.find((f) => f.id === view.folderId)?.name ?? "フォルダ";
    case "tags":
      return view.tags.map((t) => `#${t}`).join(" + ");
  }
}

export function Toolbar(props: ToolbarProps) {
  const {
    view,
    folders,
    itemCount,
    canGoBack,
    canGoForward,
    onGoBack,
    onGoForward,
    thumbSize,
    onThumbSizeChange,
    fitMode,
    onToggleFitMode,
    sortBy,
    onSortByChange,
    searchText,
    onSearchTextChange,
    filterOpen,
    onToggleFilterOpen,
    minRating,
    onMinRatingChange,
    mimeTypes,
    onToggleMimeType,
    onRemoveTag,
    onClearAllFilters,
    inspectorOpen,
    onToggleInspector,
    showMenuButton = false,
    mobileDrawerOpen = false,
    onToggleMobileDrawer,
    isTouchDevice = false,
    isWebClient = false,
    selectionMode = false,
    onToggleSelectionMode,
    onPlainExportFolder,
  } = props;

  const hasActiveConditions =
    minRating !== null ||
    mimeTypes.length > 0 ||
    searchText.trim().length > 0 ||
    view.kind !== "all";

  const hasExtraFilter = minRating !== null || mimeTypes.length > 0;

  return (
    <header className="toolbar-header">
      <div className="toolbar">
        <div className="toolbar-left">
          {showMenuButton && onToggleMobileDrawer && (
            <button
              type="button"
              className="icon-btn mobile-menu-btn"
              aria-label="ライブラリメニューを開く"
              aria-controls="sidebar-drawer"
              aria-expanded={mobileDrawerOpen}
              onClick={onToggleMobileDrawer}
            >
              <Menu size={17} />
            </button>
          )}
          <button
            type="button"
            className="icon-btn"
            disabled={!canGoBack}
            aria-label="戻る"
            onClick={onGoBack}
          >
            <ChevronLeft size={16} />
          </button>
          <button
            type="button"
            className="icon-btn"
            disabled={!canGoForward}
            aria-label="進む"
            onClick={onGoForward}
          >
            <ChevronRight size={16} />
          </button>
          <span className="toolbar-title">{viewTitle(view, folders)}</span>
          <span className="toolbar-count tabular-nums">{itemCount}</span>
          {view.kind === "folder" && onPlainExportFolder && (
            <button
              type="button"
              className="icon-btn"
              title={
                isWebClient
                  ? "このフォルダを ZIP でダウンロード"
                  : "このフォルダをファイルとして書き出し..."
              }
              aria-label={
                isWebClient
                  ? "このフォルダを ZIP でダウンロード"
                  : "このフォルダをファイルとして書き出し"
              }
              onClick={() => onPlainExportFolder(view.folderId)}
            >
              <Download size={14} />
            </button>
          )}
        </div>

        <div className="toolbar-center">
          <input
            type="range"
            min={120}
            max={340}
            step={4}
            value={thumbSize}
            className="zoom-slider"
            aria-label="サムネイル表示サイズ"
            onChange={(e) => onThumbSizeChange(Number(e.target.value))}
          />
        </div>

        <div className="toolbar-right">
          <button
            type="button"
            className={`icon-btn ${fitMode === "square" ? "active" : ""}`}
            title={
              fitMode === "ratio"
                ? "比率を維持して表示（クリックで正方形に切替）"
                : "正方形で表示（クリックで元の比率に切替）"
            }
            aria-label="サムネイル表示比率の切替"
            onClick={onToggleFitMode}
          >
            {fitMode === "ratio" ? <LayoutGrid size={15} /> : <Square size={15} />}
          </button>

          <select
            className="toolbar-select"
            aria-label="並び替え"
            value={sortBy}
            onChange={(e) => onSortByChange(e.target.value as SortBy)}
          >
            <option value="createdDesc">追加日（新しい順）</option>
            <option value="createdAsc">追加日（古い順）</option>
            <option value="ratingDesc">評価（高い順）</option>
            <option value="sizeDesc">サイズ（大きい順）</option>
            <option value="titleAsc">タイトル順</option>
          </select>

          <button
            type="button"
            className={`icon-btn ${filterOpen ? "active" : ""} ${hasExtraFilter ? "filter-dot" : ""}`}
            title="詳細フィルター"
            aria-label="詳細フィルターを開閉"
            onClick={onToggleFilterOpen}
          >
            <SlidersHorizontal size={15} />
          </button>

          <div className="search-pill">
            <Search size={13} style={{ color: "var(--text-muted)", flexShrink: 0 }} />
            <input
              type="text"
              aria-label="タグ・タイトル・メモを検索"
              placeholder="検索"
              value={searchText}
              onChange={(e) => onSearchTextChange(e.target.value)}
            />
            {searchText && (
              <button
                type="button"
                className="tag-chip-remove"
                aria-label="検索をクリア"
                onClick={() => onSearchTextChange("")}
              >
                <X size={12} />
              </button>
            )}
          </div>

          {isTouchDevice && onToggleSelectionMode && (
            <button
              type="button"
              className={`icon-btn ${selectionMode ? "active" : ""}`}
              title={selectionMode ? "選択モードを終了" : "複数選択モード"}
              aria-label="複数選択モードを切替"
              aria-pressed={selectionMode}
              onClick={onToggleSelectionMode}
            >
              <CheckSquare size={15} />
            </button>
          )}

          <button
            type="button"
            className={`icon-btn ${inspectorOpen ? "active" : ""}`}
            title="詳細パネルを開閉"
            aria-label="詳細パネルを開閉"
            onClick={onToggleInspector}
          >
            <PanelRight size={15} />
          </button>
        </div>
      </div>

      {filterOpen && (
        <div className="filter-bar" aria-label="詳細フィルター">
          <div className="filter-group">
            <span className="filter-group-label">評価</span>
            <button
              type="button"
              className={`pill-btn ${minRating === null ? "active" : ""}`}
              onClick={() => onMinRatingChange(null)}
            >
              すべて
            </button>
            {[1, 2, 3, 4, 5].map((star) => (
              <button
                key={star}
                type="button"
                className={`pill-btn ${minRating === star ? "active" : ""}`}
                onClick={() => onMinRatingChange(minRating === star ? null : star)}
              >
                ★{star}
                {star < 5 ? "+" : ""}
              </button>
            ))}
          </div>

          <div className="filter-group">
            <span className="filter-group-label">形式</span>
            {MIME_FILTERS.map((item) => {
              const active = mimeTypes.includes(item.mime);
              return (
                <button
                  key={item.mime}
                  type="button"
                  className={`pill-btn ${active ? "active" : ""}`}
                  onClick={() => onToggleMimeType(item.mime)}
                >
                  {item.label}
                </button>
              );
            })}
          </div>

          {view.kind === "tags" && view.tags.length > 0 && (
            <div className="filter-group">
              <span className="filter-group-label">タグ</span>
              {view.tags.map((t) => (
                <span key={t} className="tag-chip">
                  #{t}
                  <button
                    type="button"
                    className="tag-chip-remove"
                    aria-label={`${t} を絞り込みから外す`}
                    onClick={() => onRemoveTag(t)}
                  >
                    <X size={11} />
                  </button>
                </span>
              ))}
            </div>
          )}

          {hasActiveConditions && (
            <button
              type="button"
              className="pill-btn"
              style={{ marginInlineStart: "auto" }}
              onClick={onClearAllFilters}
            >
              条件をリセット
            </button>
          )}
        </div>
      )}
    </header>
  );
}
