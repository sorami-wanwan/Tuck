import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronRight } from "lucide-react";

export interface ContextMenuItem {
  id: string;
  label: string;
  icon?: ReactNode;
  checked?: boolean;
  danger?: boolean;
  disabled?: boolean;
  separatorBefore?: boolean;
  onClick?: () => void;
  children?: ContextMenuItem[];
}

interface ContextMenuProps {
  open: boolean;
  x: number;
  y: number;
  items: ContextMenuItem[];
  onClose: () => void;
}

export function ContextMenu({ open, x, y, items, onClose }: ContextMenuProps) {
  const anchorRef = useRef<HTMLSpanElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const submenuRef = useRef<HTMLDivElement>(null);
  const [portalTarget, setPortalTarget] = useState<Element | null>(null);
  const [pos, setPos] = useState({ left: x, top: y });
  const [openSubId, setOpenSubId] = useState<string | null>(null);
  const [submenuFlipLeft, setSubmenuFlipLeft] = useState(false);
  const [submenuOffsetY, setSubmenuOffsetY] = useState(0);

  useLayoutEffect(() => {
    if (!open) {
      setPortalTarget(null);
      return;
    }
    const parentDialog = anchorRef.current?.closest("dialog[open]");
    setPortalTarget(parentDialog ?? document.body);
  }, [open]);

  useLayoutEffect(() => {
    if (!open || !portalTarget || !menuRef.current) return;
    const rect = menuRef.current.getBoundingClientRect();
    const clampedX = Math.min(x, window.innerWidth - rect.width - 8);
    const clampedY = Math.min(y, window.innerHeight - rect.height - 8);
    setPos({
      left: Math.max(8, clampedX),
      top: Math.max(8, clampedY),
    });
    setOpenSubId(null);
    setSubmenuFlipLeft(false);
    setSubmenuOffsetY(0);
  }, [open, portalTarget, x, y, items]);

  useLayoutEffect(() => {
    if (!open || !openSubId || !submenuRef.current) {
      setSubmenuFlipLeft(false);
      setSubmenuOffsetY(0);
      return;
    }
    const subEl = submenuRef.current;
    const wrapEl = subEl.parentElement;
    if (!wrapEl) return;
    const wrapRect = wrapEl.getBoundingClientRect();
    const subRect = subEl.getBoundingClientRect();
    const overflowsRight = wrapRect.right + 4 + subRect.width > window.innerWidth - 8;
    const fitsLeft = wrapRect.left - 4 - subRect.width >= 8;
    setSubmenuFlipLeft(overflowsRight && fitsLeft);

    const overflowBottom = wrapRect.top + subRect.height - (window.innerHeight - 8);
    if (overflowBottom > 0) {
      setSubmenuOffsetY(Math.max(-wrapRect.top + 8, -overflowBottom));
    } else {
      setSubmenuOffsetY(0);
    }
  }, [open, openSubId, pos.left, pos.top]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (e: PointerEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose();
      }
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [open, onClose]);

  const menuElement =
    open && items.length > 0 ? (
      <div
        ref={menuRef}
        role="menu"
        className="context-menu"
        style={{ insetInlineStart: pos.left, insetBlockStart: pos.top }}
      >
        {items.map((item) => (
          <div key={item.id}>
            {item.separatorBefore && <div className="context-menu-sep" />}
            {item.children && item.children.length > 0 ? (
              <div
                className="context-submenu-wrap"
                onMouseEnter={() => {
                  setSubmenuFlipLeft(false);
                  setSubmenuOffsetY(0);
                  setOpenSubId(item.id);
                }}
                onMouseLeave={() => setOpenSubId((cur) => (cur === item.id ? null : cur))}
              >
                <button
                  type="button"
                  role="menuitem"
                  className="context-menu-item"
                  disabled={item.disabled}
                  onClick={() => {
                    setSubmenuFlipLeft(false);
                    setSubmenuOffsetY(0);
                    setOpenSubId((cur) => (cur === item.id ? null : item.id));
                  }}
                >
                  {item.icon}
                  <span style={{ flex: 1 }}>{item.label}</span>
                  <ChevronRight size={13} style={{ color: "var(--text-muted)" }} />
                </button>
                {openSubId === item.id && (
                  <div
                    ref={submenuRef}
                    className={`context-submenu${submenuFlipLeft ? " open-left" : ""}`}
                    style={submenuOffsetY !== 0 ? { insetBlockStart: submenuOffsetY } : undefined}
                    role="menu"
                  >
                    {item.children.map((sub) => (
                      <div key={sub.id}>
                        {sub.separatorBefore && <div className="context-menu-sep" />}
                        <button
                          type="button"
                          role="menuitem"
                          className={`context-menu-item ${sub.danger ? "danger" : ""}`}
                          disabled={sub.disabled}
                          onClick={() => {
                            sub.onClick?.();
                            onClose();
                          }}
                        >
                          {sub.icon}
                          <span
                            style={{
                              flex: 1,
                              overflow: "hidden",
                              textOverflow: "ellipsis",
                              whiteSpace: "nowrap",
                            }}
                          >
                            {sub.label}
                          </span>
                          {sub.checked && (
                            <Check
                              size={12}
                              aria-hidden="true"
                              style={{ flexShrink: 0, color: "var(--accent-blue)" }}
                            />
                          )}
                        </button>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            ) : (
              <button
                type="button"
                role="menuitem"
                className={`context-menu-item ${item.danger ? "danger" : ""}`}
                disabled={item.disabled}
                onClick={() => {
                  item.onClick?.();
                  onClose();
                }}
              >
                {item.icon}
                <span>{item.label}</span>
              </button>
            )}
          </div>
        ))}
      </div>
    ) : null;

  return (
    <>
      <span ref={anchorRef} hidden />
      {menuElement && portalTarget ? createPortal(menuElement, portalTarget) : null}
    </>
  );
}
