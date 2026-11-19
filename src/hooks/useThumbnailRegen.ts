import { useCallback, useState } from "react";
import { imageMemoryCache } from "../services/imageCache";
import { formatErrorMessage, vaultApi } from "../services/vaultApi";
import type { EntryRef, ItemDetail, ToastItem } from "../types/vault";

interface UseThumbnailRegenOptions {
  canEdit: boolean;
  refreshLibrary: () => Promise<void>;
  refreshActiveDetail: () => Promise<void>;
  onItemDetailUpdated: (detail: ItemDetail) => void;
  pushToast: (tone: ToastItem["tone"], message: string) => void;
}

/**
 * 単体・複数・グループ選択項目のサムネイル再生成と動画現在フレームのキャプチャを管理するフック。
 */
export function useThumbnailRegen({
  canEdit,
  refreshLibrary,
  refreshActiveDetail,
  onItemDetailUpdated,
  pushToast,
}: UseThumbnailRegenOptions) {
  const [regenerating, setRegenerating] = useState(false);

  const regenerateEntries = useCallback(
    async (refs: EntryRef[]) => {
      if (!canEdit || refs.length === 0 || regenerating) return;
      setRegenerating(true);
      try {
        const targets = await vaultApi.listThumbnailTargets("entries", refs);
        if (targets.length === 0) {
          pushToast("info", "再生成できる対象がありません");
          return;
        }
        if (targets.length >= 2) {
          pushToast("info", `${targets.length}件のサムネイルを再生成しています...`);
        }

        let succeeded = 0;
        let lastError: string | null = null;
        let lastUpdated: ItemDetail | null = null;
        for (const t of targets) {
          try {
            const updated = await imageMemoryCache.regenerateItemThumbnail(
              t.itemId,
              t.mimeType
            );
            succeeded += 1;
            lastUpdated = updated;
          } catch (err) {
            lastError = formatErrorMessage(err);
          }
        }

        if (targets.length === 1 && lastUpdated) {
          onItemDetailUpdated(lastUpdated);
        }
        await Promise.all([refreshLibrary(), refreshActiveDetail()]);

        if (succeeded === targets.length) {
          pushToast(
            "success",
            targets.length === 1
              ? "サムネイルを再生成しました"
              : `${succeeded}件のサムネイルを再生成しました`
          );
        } else if (succeeded > 0) {
          pushToast(
            "warning",
            `${targets.length}件中 ${succeeded}件のサムネイルを再生成しました（${
              targets.length - succeeded
            }件失敗）`
          );
        } else {
          pushToast("error", lastError ?? "サムネイルの再生成に失敗しました");
        }
      } catch (err) {
        pushToast("error", formatErrorMessage(err));
      } finally {
        setRegenerating(false);
      }
    },
    [
      canEdit,
      regenerating,
      onItemDetailUpdated,
      refreshLibrary,
      refreshActiveDetail,
      pushToast,
    ]
  );

  const captureCurrentVideoFrame = useCallback(
    async (itemId: string, videoEl: HTMLVideoElement | null) => {
      if (!canEdit || !videoEl || regenerating) return;
      setRegenerating(true);
      try {
        const timestampMs = Math.max(0, Math.round((videoEl.currentTime || 0) * 1000));
        const updated = await imageMemoryCache.captureVideoThumbnailAt(
          itemId,
          videoEl,
          timestampMs
        );
        onItemDetailUpdated(updated);
        await refreshLibrary();
        pushToast("success", "現在のフレームをサムネイルに設定しました");
      } catch (err) {
        pushToast("error", formatErrorMessage(err));
      } finally {
        setRegenerating(false);
      }
    },
    [canEdit, regenerating, onItemDetailUpdated, refreshLibrary, pushToast]
  );

  return {
    regenerating,
    regenerateEntries,
    captureCurrentVideoFrame,
  };
}
