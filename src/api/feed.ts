import { apiFetch } from "./client";
import type { FeedFilterRule, FeedResponse } from "../types/api";

export async function getFeed(url: string): Promise<FeedResponse> {
  return apiFetch<FeedResponse>(`/api/feed?url=${encodeURIComponent(url)}`);
}

export async function dismissFeedEntry(
  arxivId: string,
  version: number,
  permanent = false,
): Promise<void> {
  await apiFetch("/api/feed/dismiss", {
    method: "POST",
    body: JSON.stringify({ arxiv_id: arxivId, version, permanent }),
  });
}

export async function listFeedRules(): Promise<FeedFilterRule[]> {
  const res = await apiFetch<{ rules: FeedFilterRule[] }>("/api/feed/rules");
  return res.rules;
}

export async function createFeedRule(
  field: FeedFilterRule["field"],
  keywords: string,
  action: FeedFilterRule["action"] = "DENY",
): Promise<void> {
  await apiFetch("/api/feed/rules", {
    method: "POST",
    body: JSON.stringify({ field, keywords, action }),
  });
}

export async function deleteFeedRule(ruleId: number): Promise<void> {
  await apiFetch(`/api/feed/rules/${ruleId}`, { method: "DELETE" });
}
