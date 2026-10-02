/** Typed Tauri invoke helpers with browser fallback — barrel re-export. */

export {
  isTauri,
  isDesktopHost,
  hasHost,
  isMirrorClient,
  listen,
} from "./api/host";

export * from "./api/session";
export * from "./api/system";
export * from "./api/project";
export * from "./api/workspace";
export * from "./api/git";
export * from "./api/fs";
export * from "./api/settings";
export * from "./api/extensions";
export * from "./api/pluginHost";
export * from "./api/pluginApi";
export * from "./api/account";
export * from "./api/providers";
export * from "./api/mirror";
export * from "./api/automations";
export * from "./api/agents";
export * from "./api/memory";
export {
  batchAgentsHeadless,
  streamingMessagesJsonProbe,
  voiceStatus,
  voiceTranscribe,
} from "./api/voice";
export type {
  BatchAgentsHeadlessResult,
  StreamingMessagesJsonProbeResult,
  VoiceStatusDto,
  VoiceTranscribeResult,
} from "./api/voice";
export * from "./api/runtime";
export * from "./api/ssh";
export {
  listenWallpaperRemoteSearchBatch,
  listenWallpaperRemoteSearchProgress,
  wallpaperLibraryDelete,
  wallpaperLibraryFindById,
  wallpaperLibraryList,
  wallpaperLibraryLookup,
  wallpaperLibraryPage,
  wallpaperLibraryRemember,
  wallpaperRemoteCancelAllMediaRequests,
  wallpaperRemoteCancelMediaRequests,
  wallpaperRemoteFetchMedia,
  wallpaperRemoteSearch,
  wallpaperRemoteSearchCancel,
  wallpaperRemoteSearchMore,
  wallpaperRemoteThumbnail,
  xEvidenceGet,
  xEvidenceList,
  xEvidenceSearch,
  xEvidenceStats,
  xQuotePack,
} from "./api/wallpaper";
export type {
  WallpaperFetchResult,
  WallpaperGalleryItem,
  WallpaperLibraryEntry,
  WallpaperLibraryMatch,
  WallpaperLibraryPage,
  WallpaperLibraryQuery,
  XEvidenceFilter,
  XEvidenceItem,
  XEvidenceStats,
  XQuotePack,
  XSearchEnvelope,
} from "./api/wallpaper";
export * from "./api/pet";
export * from "./api/skin";
