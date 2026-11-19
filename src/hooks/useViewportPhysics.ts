import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type MouseEvent,
  type PointerEvent,
  type RefObject,
} from "react";

export interface UseViewportPhysicsOptions {
  viewportRef: RefObject<HTMLDivElement | null>;
  stageRef: RefObject<HTMLDivElement | null>;
  contentWidth: number;
  contentHeight: number;
  maxFitScale?: number;
  enabled: boolean;
  resetKey: string | number;
  onSwipeLeft?: () => void;
  onSwipeRight?: () => void;
  onTapZone?: (zone: "left" | "center" | "right") => void;
}

export interface ViewportHudState {
  scale: number;
  fitScale: number;
  isFit: boolean;
  canPan: boolean;
}

interface SafariGestureEvent extends UIEvent {
  scale: number;
  clientX: number;
  clientY: number;
}

interface PhysicsState {
  scale: number;
  x: number;
  y: number;
  vx: number;
  vy: number;
  vScale: number;
  mode: "idle" | "dragging" | "inertia" | "spring";
  targetScale: number;
  targetX: number;
  targetY: number;
}

function rubberBand(val: number, limit: number, dim: number): number {
  const abs = Math.abs(val);
  if (abs <= limit) return val;
  const overflow = abs - limit;
  const safeDim = Math.max(dim, 200);
  const damped = (overflow * 0.55 * safeDim) / (safeDim + 0.55 * overflow);
  return Math.sign(val) * (limit + damped);
}

export function useViewportPhysics({
  viewportRef,
  stageRef,
  contentWidth,
  contentHeight,
  maxFitScale = 1,
  enabled,
  resetKey,
  onSwipeLeft,
  onSwipeRight,
  onTapZone,
}: UseViewportPhysicsOptions) {
  const callbacksRef = useRef({ onSwipeLeft, onSwipeRight, onTapZone });
  callbacksRef.current = { onSwipeLeft, onSwipeRight, onTapZone };

  const [viewportSize, setViewportSize] = useState<{ width: number; height: number } | null>(null);
  const [isDragging, setIsDragging] = useState(false);
  const [hud, setHud] = useState<ViewportHudState>({
    scale: 1,
    fitScale: 1,
    isFit: true,
    canPan: false,
  });

  const vw = viewportSize?.width ?? 0;
  const vh = viewportSize?.height ?? 0;
  const safeW = Math.max(1, contentWidth);
  const safeH = Math.max(1, contentHeight);

  const fitScale =
    vw > 0 && vh > 0 ? Math.min(vw / safeW, vh / safeH, maxFitScale) : 1;
  const maxScale = Math.max(fitScale * 8, 4);

  const metricsRef = useRef({ vw, vh, safeW, safeH, fitScale, maxScale });
  metricsRef.current = { vw, vh, safeW, safeH, fitScale, maxScale };

  const stateRef = useRef<PhysicsState>({
    scale: fitScale,
    x: 0,
    y: 0,
    vx: 0,
    vy: 0,
    vScale: 0,
    mode: "idle",
    targetScale: fitScale,
    targetX: 0,
    targetY: 0,
  });

  const rafRef = useRef<number | null>(null);
  const settleTimerRef = useRef<number | null>(null);
  const lastFrameRef = useRef<number>(0);
  const lastHudRef = useRef<ViewportHudState>({
    scale: 1,
    fitScale: 1,
    isFit: true,
    canPan: false,
  });

  const dragRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    originX: number;
    originY: number;
    samples: Array<{ t: number; x: number; y: number }>;
  } | null>(null);

  const lastResetKeyRef = useRef<string | number>(resetKey);
  const gestureRef = useRef<{ prevScale: number; lastWheelPinchAt: number }>({
    prevScale: 1,
    lastWheelPinchAt: 0,
  });
  const touchPointsRef = useRef<Map<number, { x: number; y: number }>>(new Map());
  const touchGestureRef = useRef<{
    pinchPrevDist: number | null;
    pinchPrevMid: { x: number; y: number } | null;
    swipeStart: { x: number; y: number; t: number } | null;
    hadPinch: boolean;
    movedSinceDown: boolean;
    lastTap: { t: number; x: number; y: number } | null;
    tapTimer: number | null;
  }>({
    pinchPrevDist: null,
    pinchPrevMid: null,
    swipeStart: null,
    hadPinch: false,
    movedSinceDown: false,
    lastTap: null,
    tapTimer: null,
  });

  const getBounds = useCallback((scale: number) => {
    const { vw: w, vh: h, safeW: cw, safeH: ch } = metricsRef.current;
    if (w <= 0 || h <= 0) return { maxX: 0, maxY: 0 };
    return {
      maxX: Math.max(0, (cw * scale - w) / 2),
      maxY: Math.max(0, (ch * scale - h) / 2),
    };
  }, []);

  const clampPoint = useCallback(
    (scale: number, x: number, y: number) => {
      const { fitScale: minS, maxScale: maxS } = metricsRef.current;
      const clampedScale = Math.max(minS, Math.min(maxS, scale));
      const { maxX, maxY } = getBounds(clampedScale);
      return {
        scale: clampedScale,
        x: Math.max(-maxX, Math.min(maxX, x)),
        y: Math.max(-maxY, Math.min(maxY, y)),
      };
    },
    [getBounds]
  );

  const commitTransform = useCallback(() => {
    const s = stateRef.current;
    const stage = stageRef.current;
    if (stage) {
      stage.style.transform = `translate3d(${s.x.toFixed(2)}px, ${s.y.toFixed(2)}px, 0) scale(${s.scale.toFixed(5)})`;
    }

    const { fitScale: fs } = metricsRef.current;
    const { maxX, maxY } = getBounds(s.scale);
    const isFit = Math.abs(s.scale - fs) < Math.max(0.004, fs * 0.012);
    const canPan = maxX > 0.5 || maxY > 0.5;

    const prev = lastHudRef.current;
    if (
      Math.round(prev.scale * 1000) !== Math.round(s.scale * 1000) ||
      Math.round(prev.fitScale * 1000) !== Math.round(fs * 1000) ||
      prev.isFit !== isFit ||
      prev.canPan !== canPan
    ) {
      const next = { scale: s.scale, fitScale: fs, isFit, canPan };
      lastHudRef.current = next;
      setHud(next);
    }
  }, [getBounds, stageRef]);

  const stopAnimation = useCallback(() => {
    if (rafRef.current !== null) {
      cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    }
    if (settleTimerRef.current !== null) {
      window.clearTimeout(settleTimerRef.current);
      settleTimerRef.current = null;
    }
  }, []);

  const stepAnimation = useCallback(
    (now: number) => {
      const s = stateRef.current;
      const rawDt = (now - lastFrameRef.current) / 1000;
      lastFrameRef.current = now;
      const dt = Math.min(0.05, Math.max(0.001, rawDt));

      if (s.mode === "spring") {
        const subSteps = dt > 0.016 ? 2 : 1;
        const h = dt / subSteps;
        const omega = 24;
        const k = omega * omega;
        const c = 2 * omega;

        for (let i = 0; i < subSteps; i++) {
          const ax = k * (s.targetX - s.x) - c * s.vx;
          const ay = k * (s.targetY - s.y) - c * s.vy;
          const as = k * (s.targetScale - s.scale) - c * s.vScale;
          s.vx += ax * h;
          s.vy += ay * h;
          s.vScale += as * h;
          s.x += s.vx * h;
          s.y += s.vy * h;
          s.scale += s.vScale * h;
        }

        commitTransform();

        const settled =
          Math.abs(s.targetX - s.x) < 0.3 &&
          Math.abs(s.targetY - s.y) < 0.3 &&
          Math.abs(s.targetScale - s.scale) < 0.0008 &&
          Math.hypot(s.vx, s.vy) < 4 &&
          Math.abs(s.vScale) < 0.01;

        if (settled) {
          s.x = s.targetX;
          s.y = s.targetY;
          s.scale = s.targetScale;
          s.vx = 0;
          s.vy = 0;
          s.vScale = 0;
          s.mode = "idle";
          commitTransform();
          rafRef.current = null;
          return;
        }
        rafRef.current = requestAnimationFrame(stepAnimation);
        return;
      }

      if (s.mode === "inertia") {
        const { maxX, maxY } = getBounds(s.scale);
        const subSteps = dt > 0.016 ? 2 : 1;
        const h = dt / subSteps;
        const friction = Math.pow(0.0022, h);
        const omega = 20;
        const k = omega * omega;
        const c = 2 * omega;

        for (let i = 0; i < subSteps; i++) {
          if (Math.abs(s.x) > maxX) {
            const edgeX = Math.sign(s.x) * maxX;
            const ax = k * (edgeX - s.x) - c * s.vx;
            s.vx += ax * h;
            s.x += s.vx * h;
          } else {
            s.x += s.vx * h;
            s.vx *= friction;
          }

          if (Math.abs(s.y) > maxY) {
            const edgeY = Math.sign(s.y) * maxY;
            const ay = k * (edgeY - s.y) - c * s.vy;
            s.vy += ay * h;
            s.y += s.vy * h;
          } else {
            s.y += s.vy * h;
            s.vy *= friction;
          }
        }

        commitTransform();

        const nearEdgeX = Math.abs(s.x) <= maxX + 0.35;
        const nearEdgeY = Math.abs(s.y) <= maxY + 0.35;
        if (nearEdgeX && nearEdgeY && Math.hypot(s.vx, s.vy) < 6) {
          s.x = Math.max(-maxX, Math.min(maxX, s.x));
          s.y = Math.max(-maxY, Math.min(maxY, s.y));
          s.vx = 0;
          s.vy = 0;
          s.mode = "idle";
          commitTransform();
          rafRef.current = null;
          return;
        }

        rafRef.current = requestAnimationFrame(stepAnimation);
        return;
      }

      rafRef.current = null;
    },
    [commitTransform, getBounds]
  );

  const animateTo = useCallback(
    (nextScale: number, nextX: number, nextY: number) => {
      stopAnimation();
      const clamped = clampPoint(nextScale, nextX, nextY);
      const s = stateRef.current;
      s.mode = "spring";
      s.targetScale = clamped.scale;
      s.targetX = clamped.x;
      s.targetY = clamped.y;
      lastFrameRef.current = performance.now();
      rafRef.current = requestAnimationFrame(stepAnimation);
    },
    [clampPoint, stepAnimation, stopAnimation]
  );

  const scheduleSettle = useCallback(
    (delayMs: number) => {
      if (settleTimerRef.current !== null) {
        window.clearTimeout(settleTimerRef.current);
      }
      settleTimerRef.current = window.setTimeout(() => {
        settleTimerRef.current = null;
        const s = stateRef.current;
        const clamped = clampPoint(s.scale, s.x, s.y);
        if (
          Math.abs(clamped.scale - s.scale) > 0.001 ||
          Math.abs(clamped.x - s.x) > 0.5 ||
          Math.abs(clamped.y - s.y) > 0.5
        ) {
          animateTo(clamped.scale, clamped.x, clamped.y);
        }
      }, delayMs);
    },
    [animateTo, clampPoint]
  );

  const applyAnchoredZoom = useCallback(
    (factor: number, clientX: number, clientY: number) => {
      const vp = viewportRef.current;
      if (!vp) return;
      const rect = vp.getBoundingClientRect();
      const ax = clientX - (rect.left + rect.width / 2);
      const ay = clientY - (rect.top + rect.height / 2);

      stopAnimation();
      const s = stateRef.current;
      const { fitScale: fs, maxScale: ms } = metricsRef.current;
      const softMin = fs * 0.65;
      const softMax = ms * 1.15;
      const nextScale = Math.max(softMin, Math.min(softMax, s.scale * factor));
      const ratio = nextScale / Math.max(s.scale, 1e-6);

      s.mode = "idle";
      s.vx = 0;
      s.vy = 0;
      s.vScale = 0;
      s.x = ax - (ax - s.x) * ratio;
      s.y = ay - (ay - s.y) * ratio;
      s.scale = nextScale;

      if (nextScale <= fs) {
        s.x *= 0.85;
        s.y *= 0.85;
      }

      commitTransform();
      scheduleSettle(130);
    },
    [commitTransform, scheduleSettle, stopAnimation, viewportRef]
  );

  // Measure viewport available space
  useEffect(() => {
    const el = viewportRef.current;
    if (!el) return;
    const observer = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect;
      if (rect && rect.width > 0 && rect.height > 0) {
        setViewportSize({ width: rect.width, height: rect.height });
      }
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [viewportRef]);

  // Synchronize initial / resized fit synchronously before paint
  useLayoutEffect(() => {
    if (!enabled) return;
    const s = stateRef.current;
    const resetChanged = lastResetKeyRef.current !== resetKey;
    if (resetChanged) {
      lastResetKeyRef.current = resetKey;
      stopAnimation();
      s.mode = "idle";
      s.scale = fitScale;
      s.x = 0;
      s.y = 0;
      s.vx = 0;
      s.vy = 0;
      s.vScale = 0;
      commitTransform();
      return;
    }

    if (lastHudRef.current.isFit && s.mode === "idle") {
      s.scale = fitScale;
      s.x = 0;
      s.y = 0;
    } else if (s.mode === "idle") {
      const clamped = clampPoint(s.scale, s.x, s.y);
      s.scale = clamped.scale;
      s.x = clamped.x;
      s.y = clamped.y;
    }
    commitTransform();
  }, [enabled, fitScale, contentWidth, contentHeight, resetKey, clampPoint, commitTransform, stopAnimation]);

  // Native non-passive wheel + Safari gesture listeners for trackpad pinch & 2-finger pan
  useEffect(() => {
    const vp = viewportRef.current;
    if (!vp || !enabled) return;

    const onWheel = (e: WheelEvent) => {
      e.preventDefault();

      if (e.ctrlKey || e.metaKey) {
        gestureRef.current.lastWheelPinchAt = performance.now();
        const clampedDelta = Math.max(-120, Math.min(120, e.deltaY));
        const factor = Math.exp(-clampedDelta * 0.01);
        applyAnchoredZoom(factor, e.clientX, e.clientY);
        return;
      }

      const s = stateRef.current;
      const { vw: w, vh: h } = metricsRef.current;
      const { maxX, maxY } = getBounds(s.scale);
      if (maxX <= 0.5 && maxY <= 0.5) return;

      stopAnimation();
      s.mode = "idle";
      s.vx = 0;
      s.vy = 0;
      const nextX = s.x - e.deltaX;
      const nextY = s.y - e.deltaY;
      s.x = rubberBand(nextX, maxX, w);
      s.y = rubberBand(nextY, maxY, h);
      commitTransform();
      scheduleSettle(130);
    };

    const onGestureStart = (e: Event) => {
      e.preventDefault();
      gestureRef.current.prevScale = 1;
    };

    const onGestureChange = (e: Event) => {
      e.preventDefault();
      const ge = e as SafariGestureEvent;
      if (performance.now() - gestureRef.current.lastWheelPinchAt < 60) {
        gestureRef.current.prevScale = ge.scale || 1;
        return;
      }
      const cur = ge.scale || 1;
      const factor = cur / Math.max(gestureRef.current.prevScale, 0.01);
      gestureRef.current.prevScale = cur;
      applyAnchoredZoom(factor, ge.clientX, ge.clientY);
    };

    const onGestureEnd = (e: Event) => {
      e.preventDefault();
      gestureRef.current.prevScale = 1;
      scheduleSettle(40);
    };

    vp.addEventListener("wheel", onWheel, { passive: false });
    vp.addEventListener("gesturestart", onGestureStart, { passive: false });
    vp.addEventListener("gesturechange", onGestureChange, { passive: false });
    vp.addEventListener("gestureend", onGestureEnd, { passive: false });

    return () => {
      vp.removeEventListener("wheel", onWheel);
      vp.removeEventListener("gesturestart", onGestureStart);
      vp.removeEventListener("gesturechange", onGestureChange);
      vp.removeEventListener("gestureend", onGestureEnd);
    };
  }, [enabled, viewportRef, applyAnchoredZoom, getBounds, commitTransform, scheduleSettle, stopAnimation]);

  useEffect(() => {
    return () => {
      stopAnimation();
      if (touchGestureRef.current.tapTimer !== null) {
        window.clearTimeout(touchGestureRef.current.tapTimer);
        touchGestureRef.current.tapTimer = null;
      }
    };
  }, [stopAnimation]);

  const toggleZoomAtPoint = useCallback(
    (clientX: number, clientY: number) => {
      const vp = viewportRef.current;
      if (!vp) return;
      const s = stateRef.current;
      const { fitScale: fs, maxScale: ms } = metricsRef.current;

      if (s.scale > fs * 1.06) {
        animateTo(fs, 0, 0);
        return;
      }

      const targetScale = 1 > fs * 1.18 ? Math.min(ms, 1) : Math.min(ms, fs * 2.2);
      const rect = vp.getBoundingClientRect();
      const ax = clientX - (rect.left + rect.width / 2);
      const ay = clientY - (rect.top + rect.height / 2);
      const ratio = targetScale / Math.max(s.scale, 1e-6);
      const targetX = ax - (ax - s.x) * ratio;
      const targetY = ay - (ay - s.y) * ratio;
      animateTo(targetScale, targetX, targetY);
    },
    [animateTo, viewportRef]
  );

  const onPointerDown = useCallback(
    (e: PointerEvent<HTMLDivElement>) => {
      if ((e.target as HTMLElement).closest("button")) return;

      if (e.pointerType === "touch") {
        touchPointsRef.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
        const tg = touchGestureRef.current;

        if (touchPointsRef.current.size >= 2) {
          dragRef.current = null;
          setIsDragging(false);
          stopAnimation();
          if (tg.tapTimer !== null) {
            window.clearTimeout(tg.tapTimer);
            tg.tapTimer = null;
          }
          const pts = Array.from(touchPointsRef.current.values());
          tg.pinchPrevDist = Math.hypot(pts[1].x - pts[0].x, pts[1].y - pts[0].y);
          tg.pinchPrevMid = {
            x: (pts[0].x + pts[1].x) / 2,
            y: (pts[0].y + pts[1].y) / 2,
          };
          tg.hadPinch = true;
          tg.movedSinceDown = true;
          try {
            e.currentTarget.setPointerCapture(e.pointerId);
          } catch {
            // ignore
          }
          return;
        }

        tg.hadPinch = false;
        tg.movedSinceDown = false;
        tg.pinchPrevDist = null;
        tg.pinchPrevMid = null;
        tg.swipeStart = { x: e.clientX, y: e.clientY, t: performance.now() };
        try {
          e.currentTarget.setPointerCapture(e.pointerId);
        } catch {
          // ignore
        }

        const s = stateRef.current;
        const { fitScale: fs } = metricsRef.current;
        const { maxX, maxY } = getBounds(s.scale);
        if (maxX > 0.5 || maxY > 0.5 || s.scale > fs * 1.02) {
          stopAnimation();
          s.mode = "dragging";
          s.vx = 0;
          s.vy = 0;
          setIsDragging(true);
          dragRef.current = {
            pointerId: e.pointerId,
            startX: e.clientX,
            startY: e.clientY,
            originX: s.x,
            originY: s.y,
            samples: [{ t: performance.now(), x: s.x, y: s.y }],
          };
        }
        return;
      }

      if (e.button !== 0) return;

      const s = stateRef.current;
      const { fitScale: fs } = metricsRef.current;
      const { maxX, maxY } = getBounds(s.scale);
      if (maxX <= 0.5 && maxY <= 0.5 && s.scale <= fs * 1.02) {
        return;
      }

      stopAnimation();
      e.currentTarget.setPointerCapture(e.pointerId);
      s.mode = "dragging";
      s.vx = 0;
      s.vy = 0;
      setIsDragging(true);
      dragRef.current = {
        pointerId: e.pointerId,
        startX: e.clientX,
        startY: e.clientY,
        originX: s.x,
        originY: s.y,
        samples: [{ t: performance.now(), x: s.x, y: s.y }],
      };
    },
    [getBounds, stopAnimation]
  );

  const onPointerMove = useCallback(
    (e: PointerEvent<HTMLDivElement>) => {
      if (e.pointerType === "touch") {
        if (touchPointsRef.current.has(e.pointerId)) {
          touchPointsRef.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
        }
        const tg = touchGestureRef.current;
        if (touchPointsRef.current.size >= 2) {
          const pts = Array.from(touchPointsRef.current.values());
          const dist = Math.hypot(pts[1].x - pts[0].x, pts[1].y - pts[0].y);
          const mid = {
            x: (pts[0].x + pts[1].x) / 2,
            y: (pts[0].y + pts[1].y) / 2,
          };
          if (tg.pinchPrevDist && tg.pinchPrevDist > 4 && tg.pinchPrevMid) {
            const s = stateRef.current;
            s.x += mid.x - tg.pinchPrevMid.x;
            s.y += mid.y - tg.pinchPrevMid.y;
            const factor = dist / tg.pinchPrevDist;
            if (Number.isFinite(factor) && factor > 0.2 && factor < 5) {
              applyAnchoredZoom(factor, mid.x, mid.y);
            } else {
              commitTransform();
            }
          }
          tg.pinchPrevDist = dist;
          tg.pinchPrevMid = mid;
          return;
        }
        if (tg.swipeStart) {
          const d = Math.hypot(e.clientX - tg.swipeStart.x, e.clientY - tg.swipeStart.y);
          if (d > 8) tg.movedSinceDown = true;
        }
      }

      const drag = dragRef.current;
      if (!drag || drag.pointerId !== e.pointerId) return;

      const s = stateRef.current;
      const { vw: w, vh: h } = metricsRef.current;
      const { maxX, maxY } = getBounds(s.scale);

      const rawX = drag.originX + (e.clientX - drag.startX);
      const rawY = drag.originY + (e.clientY - drag.startY);
      s.x = rubberBand(rawX, maxX, w);
      s.y = rubberBand(rawY, maxY, h);

      const now = performance.now();
      drag.samples.push({ t: now, x: s.x, y: s.y });
      while (drag.samples.length > 2 && now - drag.samples[0].t > 90) {
        drag.samples.shift();
      }
      commitTransform();
    },
    [applyAnchoredZoom, commitTransform, getBounds]
  );

  const finishDrag = useCallback(
    (e: PointerEvent<HTMLDivElement>) => {
      if (e.pointerType === "touch") {
        const wasCancel = e.type === "pointercancel";
        touchPointsRef.current.delete(e.pointerId);
        const tg = touchGestureRef.current;

        if (touchPointsRef.current.size >= 1) {
          tg.pinchPrevDist = null;
          tg.pinchPrevMid = null;
          scheduleSettle(40);
          return;
        }

        tg.pinchPrevDist = null;
        tg.pinchPrevMid = null;

        if (tg.hadPinch) {
          tg.hadPinch = false;
          tg.swipeStart = null;
          dragRef.current = null;
          setIsDragging(false);
          scheduleSettle(40);
          return;
        }

        const drag = dragRef.current;
        if (drag && drag.pointerId === e.pointerId) {
          dragRef.current = null;
          setIsDragging(false);

          const s = stateRef.current;
          const now = performance.now();
          const first = drag.samples[0];
          const last = drag.samples[drag.samples.length - 1];
          let vx = 0;
          let vy = 0;
          if (first && last && now - last.t < 55) {
            const dtSec = (last.t - first.t) / 1000;
            if (dtSec > 0.008) {
              vx = (last.x - first.x) / dtSec;
              vy = (last.y - first.y) / dtSec;
            }
          }
          const speed = Math.hypot(vx, vy);
          const { maxX, maxY } = getBounds(s.scale);
          const outOfBounds = Math.abs(s.x) > maxX + 0.5 || Math.abs(s.y) > maxY + 0.5;
          if (speed > 25 || outOfBounds) {
            s.mode = "inertia";
            s.vx = vx;
            s.vy = vy;
            lastFrameRef.current = now;
            rafRef.current = requestAnimationFrame(stepAnimation);
          } else {
            s.mode = "idle";
          }
        }

        const start = tg.swipeStart;
        tg.swipeStart = null;
        if (wasCancel || !start) return;

        const dx = e.clientX - start.x;
        const dy = e.clientY - start.y;
        const s = stateRef.current;
        const { fitScale: fs } = metricsRef.current;
        const isFitNow = Math.abs(s.scale - fs) < Math.max(0.004, fs * 0.015);

        if (isFitNow && Math.abs(dx) >= 56 && Math.abs(dx) > Math.abs(dy) * 1.35) {
          if (dx < 0) callbacksRef.current.onSwipeLeft?.();
          else callbacksRef.current.onSwipeRight?.();
          return;
        }

        if (!tg.movedSinceDown && Math.hypot(dx, dy) < 10) {
          const now = performance.now();
          const prevTap = tg.lastTap;
          if (
            prevTap &&
            now - prevTap.t < 280 &&
            Math.hypot(e.clientX - prevTap.x, e.clientY - prevTap.y) < 36
          ) {
            if (tg.tapTimer !== null) {
              window.clearTimeout(tg.tapTimer);
              tg.tapTimer = null;
            }
            tg.lastTap = null;
            toggleZoomAtPoint(e.clientX, e.clientY);
            return;
          }

          tg.lastTap = { t: now, x: e.clientX, y: e.clientY };
          const vp = viewportRef.current;
          if (vp && callbacksRef.current.onTapZone) {
            const rect = vp.getBoundingClientRect();
            const frac = (e.clientX - rect.left) / Math.max(rect.width, 1);
            const zone: "left" | "center" | "right" =
              frac < 0.28 ? "left" : frac > 0.72 ? "right" : "center";

            if (zone !== "center" && isFitNow) {
              callbacksRef.current.onTapZone(zone);
            } else {
              if (tg.tapTimer !== null) window.clearTimeout(tg.tapTimer);
              tg.tapTimer = window.setTimeout(() => {
                tg.tapTimer = null;
                callbacksRef.current.onTapZone?.(zone);
              }, 230);
            }
          }
        }
        return;
      }

      const drag = dragRef.current;
      if (!drag || drag.pointerId !== e.pointerId) return;
      dragRef.current = null;
      setIsDragging(false);

      const s = stateRef.current;
      const now = performance.now();
      const first = drag.samples[0];
      const last = drag.samples[drag.samples.length - 1];
      let vx = 0;
      let vy = 0;
      if (first && last && now - last.t < 55) {
        const dtSec = (last.t - first.t) / 1000;
        if (dtSec > 0.008) {
          vx = (last.x - first.x) / dtSec;
          vy = (last.y - first.y) / dtSec;
        }
      }

      const speed = Math.hypot(vx, vy);
      const { maxX, maxY } = getBounds(s.scale);
      const outOfBounds = Math.abs(s.x) > maxX + 0.5 || Math.abs(s.y) > maxY + 0.5;

      if (speed > 25 || outOfBounds) {
        s.mode = "inertia";
        s.vx = vx;
        s.vy = vy;
        lastFrameRef.current = now;
        rafRef.current = requestAnimationFrame(stepAnimation);
      } else {
        s.mode = "idle";
      }
    },
    [getBounds, scheduleSettle, stepAnimation, toggleZoomAtPoint, viewportRef]
  );

  const onDoubleClick = useCallback(
    (e: MouseEvent<HTMLDivElement>) => {
      if ((e.target as HTMLElement).closest("button")) return;
      toggleZoomAtPoint(e.clientX, e.clientY);
    },
    [toggleZoomAtPoint]
  );

  const zoomIn = useCallback(() => {
    const s = stateRef.current;
    const { maxScale: ms } = metricsRef.current;
    const nextScale = Math.min(ms, s.scale * 1.35);
    const ratio = nextScale / Math.max(s.scale, 1e-6);
    animateTo(nextScale, s.x * ratio, s.y * ratio);
  }, [animateTo]);

  const zoomOut = useCallback(() => {
    const s = stateRef.current;
    const { fitScale: fs } = metricsRef.current;
    const nextScale = Math.max(fs, s.scale / 1.35);
    const ratio = nextScale / Math.max(s.scale, 1e-6);
    animateTo(nextScale, s.x * ratio, s.y * ratio);
  }, [animateTo]);

  const resetFit = useCallback(() => {
    const { fitScale: fs } = metricsRef.current;
    animateTo(fs, 0, 0);
  }, [animateTo]);

  const toggleActualSize = useCallback(() => {
    const s = stateRef.current;
    const { fitScale: fs, maxScale: ms } = metricsRef.current;
    if (s.scale > fs * 1.06) {
      animateTo(fs, 0, 0);
    } else {
      const target = 1 > fs * 1.18 ? Math.min(ms, 1) : Math.min(ms, fs * 2.2);
      animateTo(target, 0, 0);
    }
  }, [animateTo]);

  return {
    viewportSize,
    isDragging,
    hud,
    zoomIn,
    zoomOut,
    resetFit,
    toggleActualSize,
    handlers: {
      onPointerDown,
      onPointerMove,
      onPointerUp: finishDrag,
      onPointerCancel: finishDrag,
      onDoubleClick,
    },
  };
}
