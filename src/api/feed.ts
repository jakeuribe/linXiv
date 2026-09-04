import { libraryFetch } from "../stores/backend.ts";
import type { FeedFilterRule, FeedResponse } from "../types/api";

export async function getFeed(url: string): Promise<FeedResponse> {
  return libraryFetch<FeedResponse>(`/api/feed?url=${encodeURIComponent(url)}`);
}

export async function dismissFeedEntry(
  arxivId: string,
  version: number,
  permanent = false,
): Promise<void> {
  await libraryFetch("/api/feed/dismiss", {
    method: "POST",
    body: JSON.stringify({ arxiv_id: arxivId, version, permanent }),
  });
}

export async function listFeedRules(): Promise<FeedFilterRule[]> {
  const res = await libraryFetch<{ rules: FeedFilterRule[] }>("/api/feed/rules");
  return res.rules;
}

export async function createFeedRule(
  field: FeedFilterRule["field"],
  keywords: string,
  action: FeedFilterRule["action"] = "DENY",
): Promise<void> {
  await libraryFetch("/api/feed/rules", {
    method: "POST",
    body: JSON.stringify({ field, keywords, action }),
  });
}

export async function deleteFeedRule(ruleId: number): Promise<void> {
  await libraryFetch(`/api/feed/rules/${ruleId}`, { method: "DELETE" });
}
