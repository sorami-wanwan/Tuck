import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
} from "react";
import { Plus, Tags, Undo2, X } from "lucide-react";
import { formatErrorMessage } from "../services/vaultApi";
import {
  isVideoMime,
  type EntryRef,
  type GalleryEntry,
  type TagWithCount,
} from "../types/vault";
import {
  isImeComposing,
  normalizeTagKey,
  summarizeSelectionTags,
} from "../utils/bulkTags";

interface BulkTagDialogProps {
  open: boolean;
  entries: GalleryEntry[];
  allTags: TagWithCount[];
  onClose: () => void;
  onApply: (
    entries: EntryRef[],
    addTags: string[],
    removeTags: string[]
  ) => Promise<void>;
}

function isOutsideDialog(
  clientX: number,
  clientY: number,
  dlg: HTMLDialogElement
): boolean {
  const r = dlg.getBoundingClientRect();
  return (
    clientX < r.left ||
    clientX > r.right ||
    clientY < r.top ||
    clientY > r.bottom
  );
}

/**
 * 単体・複数選択されたエントリ（画像・動画・漫画グループ）のタグをまとめて編集するモーダル。
 */
export function BulkTagDialog({
  open,
  entries,
  allTags,
  onClose,
  onApply,
}: BulkTagDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const wasOpenRef = useRef(false);
  const pointerDownOutsideRef = useRef(false);

  const [snapshotEntries, setSnapshotEntries] = useState<GalleryEntry[]>([]);
  const [draft, setDraft] = useState("");
  const [pendingAdd, setPendingAdd] = useState<Map<string, string>>(
    () => new Map()
  );
  const [pendingRemove, setPendingRemove] = useState<Set<string>>(
    () => new Set()
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    const dlg = dialogRef.current;
    if (!dlg) return;

    if (open && !wasOpenRef.current) {
      setSnapshotEntries(entries);
      setDraft("");
      setPendingAdd(new Map());
      setPendingRemove(new Set());
      setBusy(false);
      setError(null);
      setNotice(null);
      if (!dlg.open) dlg.showModal();
      requestAnimationFrame(() => inputRef.current?.focus());
    } else if (!open && wasOpenRef.current) {
      setBusy(false);
      if (dlg.open) dlg.close();
    }
    wasOpenRef.current = open;
  }, [open, entries]);

  const targetRefs: EntryRef[] = useMemo(
    () => snapshotEntries.map((e) => ({ kind: e.kind, id: e.id })),
    [snapshotEntries]
  );

  const initialSummaries = useMemo(
    () => summarizeSelectionTags(snapshotEntries),
    [snapshotEntries]
  );

  const initialTagMap = useMemo(
    () => new Map(initialSummaries.map((s) => [s.key, s])),
    [initialSummaries]
  );

  const allTagMap = useMemo(
    () => new Map(allTags.map((t) => [normalizeTagKey(t.name), t.name])),
    [allTags]
  );

  const summaryParts = useMemo(() => {
    const imageCount = snapshotEntries.filter(
      (e) => e.kind === "image" && !isVideoMime(e.coverMimeType)
    ).length;
    const videoCount = snapshotEntries.filter(
      (e) => e.kind === "image" && isVideoMime(e.coverMimeType)
    ).length;
    const groupCount = snapshotEntries.filter((e) => e.kind === "group").length;

    const parts: string[] = [];
    if (imageCount > 0) parts.push(`画像 ${imageCount}件`);
    if (videoCount > 0) parts.push(`動画 ${videoCount}件`);
    if (groupCount > 0) parts.push(`漫画 ${groupCount}件`);
    return parts;
  }, [snapshotEntries]);

  const stageAdd = (rawName: string): boolean => {
    const trimmed = rawName.normalize("NFC").trim();
    if (!trimmed) return false;
    const key = trimmed.toLowerCase();
    const initial = initialTagMap.get(key);
    const canonicalName = allTagMap.get(key) ?? initial?.name ?? trimmed;

    setError(null);
    if (initial?.state === "all" && !pendingRemove.has(key)) {
      setNotice(`「${initial.name}」はすでに全項目に付与されています`);
      return false;
    }

    setNotice(null);
    if (pendingRemove.has(key)) {
      setPendingRemove((prev) => {
        const next = new Set(prev);
        next.delete(key);
        return next;
      });
      if (initial?.state === "partial") {
        setPendingAdd((prev) => {
          const next = new Map(prev);
          next.set(key, canonicalName);
          return next;
        });
      }
      return true;
    }

    setPendingAdd((prev) => {
      const next = new Map(prev);
      next.set(key, canonicalName);
      return next;
    });
    return true;
  };

  const stageRemove = (key: string) => {
    setError(null);
    setNotice(null);
    setPendingAdd((prev) => {
      if (!prev.has(key)) return prev;
      const next = new Map(prev);
      next.delete(key);
      return next;
    });
    if (initialTagMap.has(key)) {
      setPendingRemove((prev) => {
        const next = new Set(prev);
        next.add(key);
        return next;
      });
    }
  };

  const undoAdd = (key: string) => {
    setError(null);
    setNotice(null);
    setPendingAdd((prev) => {
      const next = new Map(prev);
      next.delete(key);
      return next;
    });
  };

  const undoRemove = (key: string) => {
    setError(null);
    setNotice(null);
    setPendingRemove((prev) => {
      const next = new Set(prev);
      next.delete(key);
      return next;
    });
  };

  const draftTrimmed = draft.normalize("NFC").trim();
  const draftKey = draftTrimmed.toLowerCase();

  const filteredSuggestions = useMemo(() => {
    return allTags.filter((tag) => {
      const key = normalizeTagKey(tag.name);
      if (pendingAdd.has(key)) return false;
      const initial = initialTagMap.get(key);
      if (initial?.state === "all" && !pendingRemove.has(key)) return false;
      if (draftKey && !tag.name.toLowerCase().includes(draftKey)) return false;
      return true;
    });
  }, [allTags, pendingAdd, initialTagMap, pendingRemove, draftKey]);

  const showCreateOption =
    draftTrimmed.length > 0 &&
    !allTagMap.has(draftKey) &&
    !pendingAdd.has(draftKey) &&
    !(
      initialTagMap.get(draftKey)?.state === "all" &&
      !pendingRemove.has(draftKey)
    );

  const { effectiveAddTags, effectiveRemoveTags } = useMemo(() => {
    const addMap = new Map(pendingAdd);
    const removeSet = new Set(pendingRemove);

    if (draftTrimmed) {
      const initial = initialTagMap.get(draftKey);
      const canonicalName = allTagMap.get(draftKey) ?? initial?.name ?? draftTrimmed;
      if (removeSet.has(draftKey)) {
        removeSet.delete(draftKey);
        if (initial?.state === "partial") {
          addMap.set(draftKey, canonicalName);
        }
      } else if (initial?.state !== "all") {
        addMap.set(draftKey, canonicalName);
      }
    }

    const adds = Array.from(addMap.values());
    const removes = Array.from(removeSet)
      .map((k) => initialTagMap.get(k)?.name)
      .filter((n): n is string => Boolean(n));

    return { effectiveAddTags: adds, effectiveRemoveTags: removes };
  }, [pendingAdd, pendingRemove, draftTrimmed, draftKey, initialTagMap, allTagMap]);

  const hasChanges =
    effectiveAddTags.length > 0 || effectiveRemoveTags.length > 0;

  const currentAllTags = initialSummaries.filter(
    (s) =>
      s.state === "all" && !pendingRemove.has(s.key) && !pendingAdd.has(s.key)
  );
  const currentPartialTags = initialSummaries.filter(
    (s) =>
      s.state === "partial" &&
      !pendingRemove.has(s.key) &&
      !pendingAdd.has(s.key)
  );
  const stagedAddEntries = Array.from(pendingAdd.entries());
  const stagedRemoveEntries = Array.from(pendingRemove)
    .map((k) => initialTagMap.get(k))
    .filter((s): s is NonNullable<typeof s> => Boolean(s));

  const submitApply = async () => {
    if (busy || !hasChanges || targetRefs.length === 0) return;
    setBusy(true);
    setError(null);
    try {
      await onApply(targetRefs, effectiveAddTags, effectiveRemoveTags);
      onClose();
    } catch (err) {
      setError(formatErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleFormSubmit = (e: FormEvent) => {
    e.preventDefault();
    void submitApply();
  };

  const handleInputKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (isImeComposing(e)) return;
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      void submitApply();
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      if (stageAdd(draft)) {
        setDraft("");
      }
    }
  };

  const changeSummaryParts: string[] = [];
  if (effectiveAddTags.length > 0) {
    changeSummaryParts.push(`追加 ${effectiveAddTags.length}件`);
  }
  if (effectiveRemoveTags.length > 0) {
    changeSummaryParts.push(`削除 ${effectiveRemoveTags.length}件`);
  }

  return (
    <dialog
      ref={dialogRef}
      className="confirm-dialog bulk-tag-dialog"
      aria-labelledby="bulk-tag-dialog-title"
      onCancel={(e) => {
        e.preventDefault();
        if (!busy) onClose();
      }}
      onPointerDown={(e) => {
        pointerDownOutsideRef.current = isOutsideDialog(
          e.clientX,
          e.clientY,
          e.currentTarget
        );
      }}
      onPointerUp={(e) => {
        const outside = isOutsideDialog(e.clientX, e.clientY, e.currentTarget);
        if (!busy && pointerDownOutsideRef.current && outside) {
          onClose();
        }
        pointerDownOutsideRef.current = false;
      }}
    >
      <form className="bulk-tag-form" onSubmit={handleFormSubmit}>
        <div className="maintenance-header">
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <Tags size={16} style={{ color: "var(--accent-blue)", flexShrink: 0 }} />
            <div>
              <h2 id="bulk-tag-dialog-title">
                {snapshotEntries.length >= 2
                  ? `${snapshotEntries.length}件のタグを一括編集`
                  : "タグを編集"}
              </h2>
              {summaryParts.length > 0 && (
                <div className="bulk-tag-subtitle">{summaryParts.join(" / ")}</div>
              )}
            </div>
          </div>
          <button
            type="button"
            className="icon-btn"
            aria-label="閉じる"
            disabled={busy}
            onClick={onClose}
          >
            <X size={15} />
          </button>
        </div>

        <div className="bulk-tag-body">
          {error && (
            <div className="error-banner" role="alert">
              {error}
            </div>
          )}

          {notice && (
            <div className="bulk-tag-notice" role="status">
              {notice}
            </div>
          )}

          <div className="form-field">
            <label htmlFor="bulk-tag-input">
              タグを追加（新規入力または候補から選択）
            </label>
            <div className="bulk-tag-input-row">
              <input
                ref={inputRef}
                id="bulk-tag-input"
                type="text"
                className="inspector-input"
                autoFocus
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="none"
                spellCheck={false}
                enterKeyHint="done"
                placeholder="タグ名を入力して Enter..."
                value={draft}
                disabled={busy}
                onChange={(e) => {
                  setDraft(e.target.value);
                  setNotice(null);
                  setError(null);
                }}
                onKeyDown={handleInputKeyDown}
              />
              <button
                type="button"
                className="btn"
                disabled={busy || !draftTrimmed}
                onClick={() => {
                  if (stageAdd(draft)) {
                    setDraft("");
                    inputRef.current?.focus();
                  }
                }}
              >
                <Plus size={13} />
                追加
              </button>
            </div>
          </div>

          {(showCreateOption || filteredSuggestions.length > 0) && (
            <div className="form-field">
              <span className="section-label">既存タグ候補</span>
              <div
                className="bulk-tag-suggestions"
                role="listbox"
                aria-label="追加できるタグ候補"
              >
                {showCreateOption && (
                  <button
                    type="button"
                    className="tag-suggestion-btn bulk-tag-create-option"
                    disabled={busy}
                    onClick={() => {
                      if (stageAdd(draftTrimmed)) {
                        setDraft("");
                        inputRef.current?.focus();
                      }
                    }}
                  >
                    <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                      <Plus size={12} />
                      新しいタグ「{draftTrimmed}」を作成して追加
                    </span>
                  </button>
                )}
                {filteredSuggestions.map((sug) => (
                  <button
                    key={sug.id}
                    type="button"
                    className="tag-suggestion-btn"
                    disabled={busy}
                    onClick={() => {
                      if (stageAdd(sug.name)) {
                        setDraft("");
                        inputRef.current?.focus();
                      }
                    }}
                  >
                    <span>#{sug.name}</span>
                    <span className="nav-item-count tabular-nums">{sug.count}</span>
                  </button>
                ))}
              </div>
            </div>
          )}

          <div className="form-field">
            <span className="section-label">選択中のタグ状態</span>
            <div className="tag-chips-wrap">
              {stagedAddEntries.length === 0 &&
                currentAllTags.length === 0 &&
                currentPartialTags.length === 0 &&
                stagedRemoveEntries.length === 0 && (
                  <span style={{ fontSize: 12, color: "var(--text-muted)" }}>
                    付与されているタグはありません
                  </span>
                )}

              {stagedAddEntries.map(([key, name]) => (
                <span key={`add-${key}`} className="tag-chip tag-chip-staged-add">
                  <Plus size={11} />
                  <span>{name}</span>
                  <button
                    type="button"
                    className="tag-chip-remove"
                    disabled={busy}
                    title="追加予定を取り消す"
                    aria-label={`${name} の追加予定を取り消す`}
                    onClick={() => undoAdd(key)}
                  >
                    <X size={11} />
                  </button>
                </span>
              ))}

              {currentAllTags.map((tag) => (
                <span key={`all-${tag.key}`} className="tag-chip">
                  <span>{tag.name}</span>
                  <button
                    type="button"
                    className="tag-chip-remove"
                    disabled={busy}
                    title="全項目から削除"
                    aria-label={`${tag.name} を削除予定にする`}
                    onClick={() => stageRemove(tag.key)}
                  >
                    <X size={11} />
                  </button>
                </span>
              ))}

              {currentPartialTags.map((tag) => (
                <span
                  key={`partial-${tag.key}`}
                  className="tag-chip tag-chip-partial"
                  title={`${tag.total}件中 ${tag.count}件に付与中`}
                >
                  <span>{tag.name}</span>
                  <span className="tag-chip-ratio tabular-nums">
                    {tag.count}/{tag.total}
                  </span>
                  <button
                    type="button"
                    className="tag-chip-apply"
                    disabled={busy}
                    title="全項目に揃える"
                    aria-label={`${tag.name} を全項目に揃える`}
                    onClick={() => stageAdd(tag.name)}
                  >
                    <Plus size={11} />
                  </button>
                  <button
                    type="button"
                    className="tag-chip-remove"
                    disabled={busy}
                    title="全項目から外す"
                    aria-label={`${tag.name} を全項目から削除予定にする`}
                    onClick={() => stageRemove(tag.key)}
                  >
                    <X size={11} />
                  </button>
                </span>
              ))}

              {stagedRemoveEntries.map((tag) => (
                <span key={`rem-${tag.key}`} className="tag-chip tag-chip-staged-remove">
                  <span>{tag.name}</span>
                  <button
                    type="button"
                    className="tag-chip-remove"
                    disabled={busy}
                    title="削除を取り消す"
                    aria-label={`${tag.name} の削除を取り消す`}
                    onClick={() => undoRemove(tag.key)}
                  >
                    <Undo2 size={11} />
                  </button>
                </span>
              ))}
            </div>
          </div>
        </div>

        <div className="bulk-tag-footer">
          <span className="bulk-tag-summary">
            {changeSummaryParts.length > 0
              ? changeSummaryParts.join(" / ")
              : "変更なし"}
          </span>
          <div style={{ display: "flex", gap: 8 }}>
            <button
              type="button"
              className="btn"
              disabled={busy}
              onClick={onClose}
            >
              キャンセル
            </button>
            <button
              type="submit"
              className="btn btn-primary"
              disabled={busy || !hasChanges || targetRefs.length === 0}
            >
              {busy ? "適用中..." : "適用する"}
            </button>
          </div>
        </div>
      </form>
    </dialog>
  );
}
