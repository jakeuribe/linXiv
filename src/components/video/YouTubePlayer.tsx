import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";

interface PlayerInstance {
  destroy(): void;
  getCurrentTime(): number;
  seekTo(seconds: number, allowSeekAhead: boolean): void;
  pauseVideo(): void;
  getVideoData(): { video_id?: string; title?: string };
  getPlaylist(): string[] | undefined;
  getPlaylistIndex(): number;
  playVideoAt(index: number): void;
  nextVideo(): void;
  previousVideo(): void;
  loadVideoById(options: { videoId: string; startSeconds: number }): void;
}

interface YouTubeApi {
  Player: new (
    element: HTMLElement,
    options: {
      videoId?: string;
      host?: string;
      playerVars?: Record<string, string | number>;
      events: {
        onReady: () => void;
        onStateChange: () => void;
        onError: (event: { data: number }) => void;
      };
    },
  ) => PlayerInstance;
}

declare global {
  interface Window {
    YT?: YouTubeApi;
    onYouTubeIframeAPIReady?: () => void;
  }
}

let apiPromise: Promise<YouTubeApi> | null = null;

function loadYouTubeApi(): Promise<YouTubeApi> {
  if (window.YT?.Player) return Promise.resolve(window.YT);
  if (apiPromise) return apiPromise;
  apiPromise = new Promise((resolve, reject) => {
    const previous = window.onYouTubeIframeAPIReady;
    window.onYouTubeIframeAPIReady = () => {
      previous?.();
      if (window.YT?.Player) resolve(window.YT);
      else reject(new Error("YouTube player API did not initialize"));
    };
    const script = document.createElement("script");
    script.src = "https://www.youtube.com/iframe_api";
    script.async = true;
    script.onerror = () => reject(new Error("Could not load the YouTube player"));
    document.head.appendChild(script);
  });
  return apiPromise;
}

export interface YouTubePlayerHandle {
  getCurrentPosition(): { timeMs: number; itemId: string | null } | null;
  seekTo(timeMs: number, itemId?: string | null): void;
  pause(): void;
}

export interface YouTubePlaylistState {
  items: string[];
  currentItemId: string | null;
  currentIndex: number;
  titles: Record<string, string>;
}

interface YouTubePlayerProps {
  videoId?: string | null;
  playlistId?: string | null;
  sourceUrl: string;
  onPlaylistStateChange?: (state: YouTubePlaylistState) => void;
}

export const YouTubePlayer = forwardRef<YouTubePlayerHandle, YouTubePlayerProps>(
  function YouTubePlayer({ videoId, playlistId, sourceUrl, onPlaylistStateChange }, ref) {
    const mountRef = useRef<HTMLDivElement>(null);
    const playerRef = useRef<PlayerInstance | null>(null);
    const pendingSeekRef = useRef<{ itemId: string; seconds: number } | null>(null);
    const titleCacheRef = useRef(new Map<string, string>());
    const [error, setError] = useState<string | null>(null);
    const [playlistItems, setPlaylistItems] = useState<string[]>([]);
    const [playlistIndex, setPlaylistIndex] = useState(0);
    const [playlistTitles, setPlaylistTitles] = useState<Record<string, string>>({});

    useEffect(() => {
      const currentItemId = playlistItems[playlistIndex] ?? null;
      onPlaylistStateChange?.({
        items: playlistItems,
        currentItemId,
        currentIndex: playlistIndex,
        titles: playlistTitles,
      });
    }, [onPlaylistStateChange, playlistIndex, playlistItems, playlistTitles]);

    useImperativeHandle(ref, () => ({
      getCurrentPosition: () => {
        const seconds = playerRef.current?.getCurrentTime();
        return seconds == null || !Number.isFinite(seconds)
          ? null
          : {
              timeMs: Math.max(0, Math.round(seconds * 1000)),
              itemId: playerRef.current?.getVideoData().video_id ?? null,
            };
      },
      seekTo: (timeMs, itemId) => {
        const player = playerRef.current;
        if (!player) return;
        const seconds = Math.max(0, timeMs) / 1000;
        const currentId = player.getVideoData().video_id;
        if (itemId && itemId !== currentId) {
          pendingSeekRef.current = { itemId, seconds };
          const playlistItemIndex = player.getPlaylist()?.indexOf(itemId) ?? -1;
          if (playlistItemIndex >= 0) player.playVideoAt(playlistItemIndex);
          else player.loadVideoById({ videoId: itemId, startSeconds: seconds });
        } else {
          player.seekTo(seconds, true);
        }
      },
      pause: () => playerRef.current?.pauseVideo(),
    }), []);

    useEffect(() => {
      let cancelled = false;
      setError(null);
      loadYouTubeApi()
        .then((YT) => {
          if (cancelled || !mountRef.current) return;
          const syncPlaylist = () => {
            const player = playerRef.current;
            if (!player) return;
            const data = player.getVideoData();
            if (data.video_id && data.title) {
              titleCacheRef.current.set(data.video_id, data.title);
              setPlaylistTitles(Object.fromEntries(titleCacheRef.current));
            }
            const items = player.getPlaylist() ?? (data.video_id ? [data.video_id] : []);
            setPlaylistItems(items);
            const index = player.getPlaylistIndex();
            setPlaylistIndex(index >= 0 ? index : Math.max(0, items.indexOf(data.video_id ?? "")));

            const pending = pendingSeekRef.current;
            if (pending && pending.itemId === data.video_id) {
              player.seekTo(pending.seconds, true);
              pendingSeekRef.current = null;
            }
          };
          playerRef.current = new YT.Player(mountRef.current, {
            videoId: videoId ?? undefined,
            host: "https://www.youtube-nocookie.com",
            playerVars: {
              controls: 1,
              rel: 0,
              playsinline: 1,
              ...(playlistId ? { listType: "playlist", list: playlistId } : {}),
            },
            events: {
              onReady: syncPlaylist,
              onStateChange: syncPlaylist,
              onError: ({ data }) => {
                const blocked = [101, 150, 153].includes(data);
                setError(blocked
                  ? "This video cannot be played inside linXiv. Open it on YouTube instead."
                  : `YouTube player error (${data}).`);
              },
            },
          });
        })
        .catch((err: unknown) => {
          if (!cancelled) setError(err instanceof Error ? err.message : "Could not load YouTube");
        });
      return () => {
        cancelled = true;
        playerRef.current?.destroy();
        playerRef.current = null;
        pendingSeekRef.current = null;
      };
    }, [videoId, playlistId]);

    return (
      <div className="h-full min-h-[280px] flex flex-col bg-black">
        <div className="flex-1 min-h-0 flex flex-col lg:flex-row">
          <div className="relative flex-1 min-h-[280px] flex items-center justify-center">
            <div ref={mountRef} className="absolute inset-0 w-full h-full" />
          </div>
          {playlistId && (
          <aside className="h-[220px] lg:h-full lg:w-[270px] shrink-0 flex flex-col bg-panel border-t lg:border-t-0 lg:border-l border-border text-text">
            <div className="px-3 py-2.5 border-b border-border flex items-center gap-2">
              <div className="min-w-0 flex-1">
                <p className="text-sm font-medium">Playlist</p>
                <p className="text-xs text-muted">
                  {playlistItems.length > 0 ? `${playlistIndex + 1} of ${playlistItems.length} lectures` : "Loading lectures…"}
                </p>
              </div>
              <button
                type="button"
                className="rounded border border-border px-2 py-1 text-xs hover:border-accent disabled:opacity-40"
                onClick={() => playerRef.current?.previousVideo()}
                disabled={playlistItems.length === 0}
                aria-label="Previous lecture"
              >
                ←
              </button>
              <button
                type="button"
                className="rounded border border-border px-2 py-1 text-xs hover:border-accent disabled:opacity-40"
                onClick={() => playerRef.current?.nextVideo()}
                disabled={playlistItems.length === 0}
                aria-label="Next lecture"
              >
                →
              </button>
            </div>
            <div className="flex-1 min-h-0 overflow-y-auto p-2 space-y-1">
              {playlistItems.map((itemId, index) => {
                const active = index === playlistIndex;
                return (
                  <button
                    type="button"
                    key={`${itemId}-${index}`}
                    onClick={() => {
                      playerRef.current?.playVideoAt(index);
                      setPlaylistIndex(index);
                    }}
                    className={`w-full rounded p-1.5 flex gap-2 text-left border transition-colors ${active ? "border-accent bg-[color-mix(in_srgb,var(--color-accent)_12%,transparent)]" : "border-transparent hover:border-border hover:bg-surface2"}`}
                    aria-current={active ? "true" : undefined}
                  >
                    <div className="relative w-24 aspect-video shrink-0 overflow-hidden rounded bg-black">
                      <img
                        src={`https://i.ytimg.com/vi/${itemId}/mqdefault.jpg`}
                        alt=""
                        className="w-full h-full object-cover"
                        loading="lazy"
                      />
                      <span className="absolute left-1 bottom-1 rounded bg-black/80 px-1 text-[10px] text-white">
                        {index + 1}
                      </span>
                    </div>
                    <span className="min-w-0 py-0.5">
                      <span className="block text-xs leading-snug line-clamp-2">
                        {playlistTitles[itemId] ?? `Lecture ${index + 1}`}
                      </span>
                      <span className="block mt-1 font-mono text-[9px] text-ink3 truncate">{itemId}</span>
                    </span>
                  </button>
                );
              })}
            </div>
          </aside>
          )}
        </div>
        {error && (
          <div className="px-4 py-3 bg-panel border-t border-border text-sm flex items-center justify-between gap-3">
            <span className="text-muted">{error}</span>
            <a className="text-accent underline shrink-0" href={sourceUrl} target="_blank" rel="noreferrer">
              Open on YouTube
            </a>
          </div>
        )}
      </div>
    );
  },
);
