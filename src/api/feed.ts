import { apiFetch } from "./client";
import type { FeedResponse } from "../types/api";

export async function getFeed(url: string): Promise<FeedResponse> {
  return apiFetch<FeedResponse>(`/api/feed?url=${encodeURIComponent(url)}`);
}
