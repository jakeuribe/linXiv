import { useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  addRemoteBackend,
  listRemoteBackends,
  remoteAvailable,
  remoteMemberCode,
  removeRemoteBackend,
} from "../../api/remote";
import { useBackendStore } from "../../stores/backend";
import { defaultAfterRemove } from "../../lib/remoteBackend";
import { errText } from "../../lib/errText";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { OptionSelect } from "../ui/select";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

const LOCAL = "__local__";

// "Remote Library Backends" (CONTEXT.md: Library Backend / Remote Query Mode):
// paste a Node Address to register a headless node, pick the PoC default
// backend, and read off this device's member code for admission. Adding never
// dials — the address is a locator, not a capability; the node's Member List
// decides access.
export function RemoteBackendsSection() {
  const queryClient = useQueryClient();
  const defaultBackend = useBackendStore((s) => s.defaultBackend);
  const setDefault = useBackendStore((s) => s.setDefault);

  const [label, setLabel] = useState("");
  const [address, setAddress] = useState("");
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const backendsQ = useQuery({
    queryKey: ["remote-backends"],
    queryFn: listRemoteBackends,
    enabled: remoteAvailable,
  });
  const memberCodeQ = useQuery({
    queryKey: ["remote-member-code"],
    queryFn: remoteMemberCode,
    enabled: remoteAvailable,
  });

  const addMutation = useMutation({
    mutationFn: () => addRemoteBackend(label.trim(), address),
    onSuccess: () => {
      setLabel("");
      setAddress("");
      void queryClient.invalidateQueries({ queryKey: ["remote-backends"] });
    },
  });

  const removeMutation = useMutation({
    mutationFn: (id: string) => removeRemoteBackend(id),
    onSuccess: (_data, id) => {
      selectDefault(defaultAfterRemove(defaultBackend, id));
      void queryClient.invalidateQueries({ queryKey: ["remote-backends"] });
    },
  });

  // Every cached view belongs to the backend it was fetched from, and query
  // keys don't carry the backend — dump the cache on a switch so remote data
  // never masquerades as local (or vice versa).
  function selectDefault(next: typeof defaultBackend) {
    if ((next?.id ?? null) === (defaultBackend?.id ?? null)) return;
    setDefault(next);
    queryClient.clear();
  }

  async function handleCopy() {
    if (!memberCodeQ.data) return;
    try {
      await navigator.clipboard.writeText(memberCodeQ.data);
      setCopied(true);
      if (copyTimer.current) clearTimeout(copyTimer.current);
      copyTimer.current = setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard write denied.
    }
  }

  if (!remoteAvailable) {
    return (
      <div>
        <SettingGroupLabel>Remote Library Backends</SettingGroupLabel>
        <SettingGroup>
          <SettingRow
            label="Remote backends"
            description="Querying a remote linXiv node runs over the desktop app's network node and isn't available in the browser preview."
          />
        </SettingGroup>
      </div>
    );
  }

  const backends = backendsQ.data ?? [];

  return (
    <div>
      <SettingGroupLabel>Remote Library Backends</SettingGroupLabel>
      <SettingGroup>
        <SettingRow
          label="Active library"
          description="Which library this app uses. Remote mode is online-only — reads and writes go to the remote node's library (per your role there). The indicator in the sidebar shows when you're not on your local library."
        >
          <OptionSelect
            aria-label="Active library backend"
            size="sm"
            options={[
              { value: LOCAL, label: "Local" },
              ...backends.map((b) => ({ value: b.id, label: b.label })),
            ]}
            value={defaultBackend?.id ?? LOCAL}
            onChange={(id) =>
              selectDefault(
                id === LOCAL ? null : (backends.find((b) => b.id === id) ?? null)
              )
            }
          />
        </SettingRow>

        {backendsQ.isError && (
          <SettingRow
            label="Registered backends"
            description={
              <span className="text-danger">
                {errText(backendsQ.error, "Failed to load the backend registry")}
              </span>
            }
          />
        )}

        {backends.map((b) => (
          <SettingRow
            key={b.id}
            label={b.label}
            description={
              <span className="font-mono break-all">{b.node_address}</span>
            }
          >
            <Button
              variant="muted"
              size="sm"
              onClick={() => removeMutation.mutate(b.id)}
              disabled={removeMutation.isPending}
            >
              Remove
            </Button>
          </SettingRow>
        ))}

        <SettingRow
          label="Add a backend"
          description={
            addMutation.isError
              ? errText(addMutation.error, "Failed to add backend")
              : "Paste the node address printed by the headless node (linxivnode…). Adding stores it — access is granted by the node's member list."
          }
        >
          <Input
            type="text"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            placeholder="Label"
            aria-label="Backend label"
            style={{ width: 110 }}
          />
          <Input
            type="text"
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            placeholder="linxivnode…"
            aria-label="Node address"
            style={{ width: 200 }}
          />
          <Button
            size="sm"
            onClick={() => addMutation.mutate()}
            disabled={addMutation.isPending || !address.trim()}
          >
            {addMutation.isPending ? "Adding…" : "Add"}
          </Button>
        </SettingRow>

        <SettingRow
          label="This device's member code"
          description={
            <>
              Send this code to the node operator — they add it to the node's
              member list to admit this device.
              {memberCodeQ.isError && (
                <span className="mt-1 block text-danger">
                  {errText(memberCodeQ.error, "Failed to read the member code")}
                </span>
              )}
              {memberCodeQ.data && (
                <span className="mt-1 block font-mono break-all text-text">
                  {memberCodeQ.data}
                </span>
              )}
            </>
          }
        >
          <Button
            variant="muted"
            size="sm"
            onClick={handleCopy}
            disabled={!memberCodeQ.data}
          >
            {copied ? "Copied" : "Copy"}
          </Button>
        </SettingRow>
      </SettingGroup>
    </div>
  );
}
