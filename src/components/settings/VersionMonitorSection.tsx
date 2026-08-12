import { useState } from "react";
import { Link } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ackNewVersion,
  checkNewVersions,
  listNewVersions,
  type NewVersion,
} from "../../api/versions";
import { ApiError } from "../../api/client";
import { Button } from "../ui/button";
import { Spinner } from "../ui/spinner";
import { MathText } from "../../lib/tex";
import { invalidatePaperMutationQueries } from "../../lib/paperMutations";
import { SettingGroup, SettingGroupLabel } from "./SettingRow";

function NewVersionRow({ item }: { item: NewVersion }) {
  const qc = useQueryClient();
  const ackMutation = useMutation({
    mutationFn: () => ackNewVersion(item.source_fk),
    onSettled: () => qc.invalidateQueries({ queryKey: ["versions", "new"] }),
  });

  return (
    <div className="flex items-center justify-between py-3 border-b border-border last:border-0">
      <div className="flex-1 min-w-0 mr-4">
        <Link
          to={`/library/${item.source_fk}`}
          className="block text-sm font-medium text-text truncate hover:underline"
        >
          <MathText forceInline>{item.title}</MathText>
        </Link>
        <p className="text-xs text-muted mt-0.5">
          New version available: v{item.version}
        </p>
        {ackMutation.isError && (
          <p className="text-xs text-danger mt-1">Failed to dismiss</p>
        )}
      </div>
      <Button
        variant="ghost"
        size="sm"
        onClick={() => ackMutation.mutate()}
        disabled={ackMutation.isPending}
        aria-label={`Dismiss ${item.title}`}
      >
        {ackMutation.isPending ? <Spinner size={14} /> : "Dismiss"}
      </Button>
    </div>
  );
}

export function VersionMonitorSection() {
  const qc = useQueryClient();
  const [lastRun, setLastRun] = useState<{ checked: number; found: number } | null>(null);

  const { data, isLoading, isError: isListError } = useQuery({
    queryKey: ["versions", "new"],
    queryFn: listNewVersions,
    staleTime: 0,
  });
  const pending = data?.new_versions ?? [];

  const checkMutation = useMutation({
    mutationFn: () => checkNewVersions(),
    onSuccess: (result) => {
      setLastRun({ checked: result.checked, found: result.new_versions.length });
      qc.invalidateQueries({ queryKey: ["versions", "new"] });
      if (result.new_versions.length > 0) {
        invalidatePaperMutationQueries(qc);
      }
    },
  });

  return (
    <div>
      <SettingGroupLabel>
        arXiv version monitoring
        {pending.length > 0 && (
          <span className="ml-2 inline-flex items-center rounded-full bg-accent/15 px-2 py-0.5 text-xs font-medium text-accent">
            {pending.length}
          </span>
        )}
      </SettingGroupLabel>
      <p className="mb-2.5 text-xs text-muted">
        Checks your saved arXiv papers for newer versions (stalest first, one
        polite request per pass) and captures anything new into the library.
      </p>
      <SettingGroup block>
        <div className="flex items-center gap-3 py-2">
          <Button
            variant="primary"
            size="sm"
            onClick={() => checkMutation.mutate()}
            disabled={checkMutation.isPending}
          >
            {checkMutation.isPending ? (
              <>
                <Spinner size={14} /> Checking…
              </>
            ) : (
              "Check for new versions"
            )}
          </Button>
          {lastRun && !checkMutation.isPending && !checkMutation.isError && (
            <span className="text-xs text-muted">
              Checked {lastRun.checked} paper{lastRun.checked !== 1 ? "s" : ""},{" "}
              {lastRun.found === 0
                ? "no new versions"
                : `found ${lastRun.found} new version${lastRun.found !== 1 ? "s" : ""}`}
            </span>
          )}
        </div>
        {checkMutation.isError && (
          <p className="text-xs text-danger mb-2">
            {checkMutation.error instanceof ApiError &&
            checkMutation.error.status === 409
              ? "A check is already running."
              : "Version check failed — arXiv may be unreachable or rate-limiting. Try again in a minute."}
          </p>
        )}
        {isListError && (
          <p className="text-xs text-danger mb-2">
            Failed to load new versions. Try refreshing the page.
          </p>
        )}
        {isLoading ? (
          <div className="flex items-center gap-2 py-3 text-sm text-muted">
            <Spinner size={14} /> Loading…
          </div>
        ) : isListError ? null : pending.length === 0 ? (
          <p className="text-sm text-muted py-2">No unreviewed new versions</p>
        ) : (
          pending.map((item) => <NewVersionRow key={item.source_fk} item={item} />)
        )}
      </SettingGroup>
    </div>
  );
}
