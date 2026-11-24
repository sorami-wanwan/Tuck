import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import { formatErrorMessage, vaultApi } from "../services/vaultApi";
import { pruneMimeTypesForMediaFilter } from "../utils/mediaFilter";
import type {
  FolderNode,
  GalleryEntry,
  MediaFilter,
  QueryScope,
  SearchQuery,
  SortBy,
  TagWithCount,
  ToastItem,
  VaultStatus,
  ViewSelection,
} from "../types/vault";

const INITIAL_VIEW: ViewSelection = { kind: "all" };

function sameViewSelection(a: ViewSelection, b: ViewSelection): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === "folder" && b.kind === "folder") return a.folderId === b.folderId;
  if (a.kind === "tags" && b.kind === "tags") {
    return a.tags.length === b.tags.length && a.tags.every((t, i) => t === b.tags[i]);
  }
  return true;
}

interface UseLibraryViewOptions {
  isOpen: boolean;
  setStatus: Dispatch<SetStateAction<VaultStatus>>;
  pushToast: (tone: ToastItem["tone"], message: string) => void;
  onSyncSelection: (nextEntries: GalleryEntry[]) => void;
  onNavigate?: () => void;
}

export function useLibraryView({
  isOpen,
  setStatus,
  pushToast,
  onSyncSelection,
  onNavigate,
}: UseLibraryViewOptions) {
  const [entries, setEntries] = useState<GalleryEntry[]>([]);
  const [folders, setFolders] = useState<FolderNode[]>([]);
  const [tags, setTags] = useState<TagWithCount[]>([]);

  const [navState, setNavState] = useState<{ stack: ViewSelection[]; index: number }>({
    stack: [INITIAL_VIEW],
    index: 0,
  });
  const view = navState.stack[navState.index] ?? INITIAL_VIEW;

  const [sidebarFilter, setSidebarFilter] = useState("");
  const [searchText, setSearchText] = useState("");
  const [sortBy, setSortBy] = useState<SortBy>("createdDesc");
  const [mediaType, setMediaType] = useState<MediaFilter>("all");
  const [minRating, setMinRating] = useState<number | null>(null);
  const [mimeTypes, setMimeTypes] = useState<string[]>([]);
  const [filterOpen, setFilterOpen] = useState(false);

  const refreshSeqRef = useRef(0);
  const onSyncSelectionRef = useRef(onSyncSelection);
  onSyncSelectionRef.current = onSyncSelection;
  const onNavigateRef = useRef(onNavigate);
  onNavigateRef.current = onNavigate;

  const currentFolderId = view.kind === "folder" ? view.folderId : null;
  const isTrashView = view.kind === "trash";
  const canGoBack = navState.index > 0;
  const canGoForward = navState.index < navState.stack.length - 1;

  const currentQuery: SearchQuery = useMemo(() => {
    const scope: QueryScope = view.kind === "tags" ? { kind: "all" } : view;
    return {
      scope,
      tagNames: view.kind === "tags" ? view.tags : [],
      minRating,
      mediaType,
      mimeTypes,
      text: searchText,
      sortBy,
    };
  }, [view, minRating, mediaType, mimeTypes, searchText, sortBy]);

  const navigateView = useCallback((next: ViewSelection) => {
    setNavState((prev) => {
      const cur = prev.stack[prev.index] ?? INITIAL_VIEW;
      if (sameViewSelection(cur, next)) return prev;
      const nextStack = [...prev.stack.slice(0, prev.index + 1), next];
      return { stack: nextStack, index: nextStack.length - 1 };
    });
    onNavigateRef.current?.();
  }, []);

  const goBack = useCallback(() => {
    setNavState((prev) => ({ ...prev, index: Math.max(0, prev.index - 1) }));
  }, []);

  const goForward = useCallback(() => {
    setNavState((prev) => ({
      ...prev,
      index: Math.min(prev.stack.length - 1, prev.index + 1),
    }));
  }, []);

  const toggleTagInView = useCallback(
    (tagName: string) => {
      const currentTags = view.kind === "tags" ? view.tags : [];
      const exists = currentTags.some((t) => t.toLowerCase() === tagName.toLowerCase());
      const nextTags = exists
        ? currentTags.filter((t) => t.toLowerCase() !== tagName.toLowerCase())
        : [...currentTags, tagName];
      if (nextTags.length === 0) {
        navigateView({ kind: "all" });
      } else {
        navigateView({ kind: "tags", tags: nextTags });
      }
    },
    [view, navigateView]
  );

  const handleMediaTypeChange = useCallback((next: MediaFilter) => {
    setMediaType(next);
    setMimeTypes((prev) => pruneMimeTypesForMediaFilter(prev, next));
  }, []);

  const toggleMimeType = useCallback((mime: string) => {
    setMimeTypes((prev) =>
      prev.includes(mime) ? prev.filter((m) => m !== mime) : [...prev, mime]
    );
  }, []);

  const hasActiveFilters =
    mediaType !== "all" ||
    minRating !== null ||
    mimeTypes.length > 0 ||
    searchText.trim().length > 0 ||
    view.kind === "tags";

  const handleResetFilters = useCallback(() => {
    setMediaType("all");
    setMinRating(null);
    setMimeTypes([]);
    setSearchText("");
    if (view.kind === "tags") {
      navigateView({ kind: "all" });
    }
  }, [view.kind, navigateView]);

  const clearAllFiltersAndView = useCallback(() => {
    handleResetFilters();
    navigateView({ kind: "all" });
  }, [handleResetFilters, navigateView]);

  const refreshLibrary = useCallback(async () => {
    if (!isOpen) return;
    const seq = ++refreshSeqRef.current;
    try {
      const [nextStatus, nextFolders, nextTags, nextEntries] = await Promise.all([
        vaultApi.getVaultStatus(),
        vaultApi.listFolders(),
        vaultApi.listTags(),
        vaultApi.queryEntries(currentQuery),
      ]);
      if (seq !== refreshSeqRef.current) return;
      setStatus(nextStatus);
      setFolders(nextFolders);
      setTags(nextTags);
      setEntries(nextEntries);
      onSyncSelectionRef.current(nextEntries);
    } catch (err) {
      if (seq !== refreshSeqRef.current) return;
      pushToast("error", formatErrorMessage(err));
    }
  }, [isOpen, currentQuery, setStatus, pushToast]);

  useEffect(() => {
    if (!isOpen) return;
    void refreshLibrary();
  }, [isOpen, refreshLibrary]);

  const resetLibraryViewState = useCallback(() => {
    setEntries([]);
    setFolders([]);
    setTags([]);
    refreshSeqRef.current += 1;
    setSearchText("");
    setSidebarFilter("");
    setMediaType("all");
    setMinRating(null);
    setMimeTypes([]);
    setNavState({ stack: [INITIAL_VIEW], index: 0 });
  }, []);

  return {
    entries,
    folders,
    tags,
    view,
    currentFolderId,
    isTrashView,
    canGoBack,
    canGoForward,
    goBack,
    goForward,
    navigateView,
    toggleTagInView,
    sidebarFilter,
    setSidebarFilter,
    searchText,
    setSearchText,
    sortBy,
    setSortBy,
    mediaType,
    handleMediaTypeChange,
    minRating,
    setMinRating,
    mimeTypes,
    toggleMimeType,
    filterOpen,
    setFilterOpen,
    hasActiveFilters,
    handleResetFilters,
    clearAllFiltersAndView,
    refreshLibrary,
    resetLibraryViewState,
  };
}
