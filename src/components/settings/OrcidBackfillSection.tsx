import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { backfillOrcids } from "../../api/orcid";
import { ApiError } from "../../api/client";
import { Button } from "../ui/button";
import { Spinner } from "../ui/spinner";
import { SettingGroup, SettingGroupLabel } from "./SettingRow";

export function OrcidBackfillSection() {
  const qc = useQueryClient();
  const [lastRun, setLastRun] = useState<{
    checked: number;
    updated: number;
    errored: number;
  } | null>(null);

  const backfillMutation = useMutation({
    mutationFn: () => backfillOrcids(),
    onSuccess: (result) => {
      setLastRun({
        checked: result.checked,
        updated: result.updated.length,
        errored: result.errored,
      });
      if (result.updated.length > 0) {
        qc.invalidateQueries({ queryKey: ["authors"] });
        result.updated.forEach((a) =>
          qc.invalidateQueries({ queryKey: ["author", a.author_id] })
        );
      }
    },
  });

  return (
    <div>
      <SettingGroupLabel>Author ORCIDs</SettingGroupLabel>
      <p className="mb-2.5 text-xs text-muted">
        New papers already pick up ORCIDs when CrossRef or OpenAlex provide
        one. This checks up to 20 existing authors that have none per click,
        via the DOI of a paper they're linked to — click again for more.
      </p>
      <SettingGroup block>
        <div className="flex items-center gap-3 py-2">
          <Button
            variant="primary"
            size="sm"
            onClick={() => backfillMutation.mutate()}
            disabled={backfillMutation.isPending}
          >
            {backfillMutation.isPending ? (
              <>
                <Spinner size={14} /> Fetching…
              </>
            ) : (
              "Fetch ORCIDs from sources"
            )}
          </Button>
          {lastRun && !backfillMutation.isPending && !backfillMutation.isError && (
            <span className="text-xs text-muted">
              Checked {lastRun.checked} author{lastRun.checked !== 1 ? "s" : ""},{" "}
              {lastRun.updated === 0
                ? "no ORCIDs found"
                : `filled ${lastRun.updated} ORCID${lastRun.updated !== 1 ? "s" : ""}`}
            </span>
          )}
        </div>
        {lastRun &&
          lastRun.errored > 0 &&
          !backfillMutation.isPending &&
          !backfillMutation.isError && (
          <p className="text-xs text-danger mb-2">
            {lastRun.errored} lookup{lastRun.errored !== 1 ? "s" : ""} failed —
            CrossRef/OpenAlex may be rate-limiting; try again shortly.
          </p>
        )}
        {backfillMutation.isError && (
          <p className="text-xs text-danger mb-2">
            {backfillMutation.error instanceof ApiError &&
            backfillMutation.error.status === 409
              ? "A backfill is already running."
              : "ORCID backfill failed — CrossRef/OpenAlex may be unreachable. Try again in a minute."}
          </p>
        )}
      </SettingGroup>
    </div>
  );
}
