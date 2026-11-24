import { useState } from "react";
import { Star, StarOff } from "lucide-react";
import type { SelectionRatingSummary } from "../utils/bulkRating";

interface BatchRatingControlProps {
  summary: SelectionRatingSummary;
  disabled?: boolean;
  showClearButton?: boolean;
  starSize?: number;
  onSelectRating: (rating: number) => void;
}

/**
 * 複数選択時の星評価コントロール（ホバープレビュー・同一星再クリック解除・任意の解除ボタン対応）。
 */
export function BatchRatingControl({
  summary,
  disabled = false,
  showClearButton = false,
  starSize = 14,
  onSelectRating,
}: BatchRatingControlProps) {
  const [hoverStar, setHoverStar] = useState<number | null>(null);
  const effectiveHover = disabled ? null : hoverStar;

  return (
    <div
      className="star-row"
      role="group"
      aria-label="一括評価"
      onMouseLeave={() => setHoverStar(null)}
    >
      {[1, 2, 3, 4, 5].map((star) => {
        const isUniformFilled = summary.uniformRating !== null && star <= summary.uniformRating;
        const isPreview = effectiveHover !== null && star <= effectiveHover;
        const showFilled = effectiveHover !== null ? isPreview : isUniformFilled;
        const willClear = summary.uniformRating === star;

        return (
          <button
            key={star}
            type="button"
            disabled={disabled}
            className={`star-btn ${showFilled ? "filled" : ""} ${isPreview ? "preview" : ""}`}
            title={willClear ? "評価を解除" : `評価を ★${star} に設定`}
            aria-label={
              willClear
                ? `星 ${star}（クリックで評価を解除）`
                : `選択項目の評価を星 ${star} に設定`
            }
            aria-pressed={summary.uniformRating === star}
            onPointerEnter={(e) => {
              if (!disabled && e.pointerType !== "touch") {
                setHoverStar(star);
              }
            }}
            onClick={() => {
              if (disabled) return;
              setHoverStar(null);
              onSelectRating(willClear ? 0 : star);
            }}
          >
            <Star size={starSize} fill={showFilled ? "currentColor" : "none"} aria-hidden="true" />
          </button>
        );
      })}
      {showClearButton && !disabled && summary.ratedCount > 0 && (
        <button
          type="button"
          className="star-clear-btn"
          disabled={disabled}
          title="評価を解除"
          aria-label="選択項目の評価を解除"
          onClick={() => {
            setHoverStar(null);
            onSelectRating(0);
          }}
        >
          <StarOff size={13} aria-hidden="true" />
        </button>
      )}
    </div>
  );
}
