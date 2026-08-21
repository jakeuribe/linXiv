import { useState } from "react";
import { useNavigate } from "react-router";
import { useMutation, useQuery } from "@tanstack/react-query";
import { reconnectRelay, shareErrText, sharingAvailable } from "../../api/share";
import { getSettings, updateSettings } from "../../api/settings";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Toggle } from "../ui/toggle";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

// Ticket generation, join-by-ticket, and the received-shares list live on
// the SharePage (/shared). This section holds device/relay-level sharing
// settings instead.
export function SharingSection() {
  const navigate = useNavigate();
  const { data: settings, isLoading } = useQuery({ queryKey: ["settings"], queryFn: getSettings });

  const [relayUrl, setRelayUrl] = useState("");
  const [relayToken, setRelayToken] = useState("");
  const [relayOnly, setRelayOnly] = useState(false);
  const [populated, setPopulated] = useState(false);
  if (settings && !populated) {
    setRelayUrl(settings.p2p_relay_url ?? "");
    setRelayToken(settings.p2p_relay_auth_token ?? "");
    setRelayOnly(settings.p2p_relay_only === true);
    setPopulated(true);
  }

  // Persists the fields above, then rebinds the p2p node against them —
  // no app restart needed (route/share.rs::reconnect_relay).
  const reconnectMutation = useMutation({
    mutationFn: async () => {
      await updateSettings({
        p2p_relay_url: relayUrl.trim(),
        p2p_relay_auth_token: relayToken.trim(),
        p2p_relay_only: relayOnly,
      });
      await reconnectRelay();
    },
  });

  if (!sharingAvailable) {
    return (
      <div>
        <SettingGroupLabel>Sharing</SettingGroupLabel>
        <SettingGroup>
          <SettingRow
            label="Project sharing"
            description="Peer-to-peer sharing runs over the desktop app's network node and isn't available in the browser preview."
          />
        </SettingGroup>
      </div>
    );
  }

  return (
    <div>
      <SettingGroupLabel>Sharing</SettingGroupLabel>
      <SettingGroup>
        <SettingRow
          label="Project sharing"
          description="Manage project sharing, invites, and members from the Sharing page."
        >
          <Button variant="primary" size="sm" onClick={() => navigate("/shared")}>
            Open Sharing
          </Button>
        </SettingRow>
        <SettingRow
          label="Relay server"
          description="Self-hosted iroh relay instead of the public n0 relays. Leave blank for the default."
        >
          <Input
            type="text"
            value={relayUrl}
            onChange={(e) => setRelayUrl(e.target.value)}
            placeholder="https://relay.example.com"
            aria-label="Custom relay URL"
            style={{ width: 240 }}
          />
        </SettingRow>
        <SettingRow
          label="Relay auth token"
          description="Bearer token for a relay configured with shared_token access. Optional."
        >
          <Input
            type="password"
            value={relayToken}
            onChange={(e) => setRelayToken(e.target.value)}
            placeholder="(optional)"
            aria-label="Relay auth token"
            style={{ width: 240 }}
          />
        </SettingRow>
        <SettingRow
          label="Only use this relay"
          description="Refuse to reconnect if this relay isn't configured or fails to bind, instead of falling back to the public n0 relay."
        >
          <Toggle
            checked={relayOnly}
            onChange={setRelayOnly}
            disabled={isLoading}
            aria-label="Only use this relay"
          />
        </SettingRow>
        <SettingRow
          label="Apply relay changes"
          description={
            reconnectMutation.isError
              ? shareErrText(reconnectMutation.error)
              : reconnectMutation.isSuccess
                ? "Reconnected."
                : "Saves the settings above and rebinds the p2p node — no app restart needed."
          }
        >
          <Button
            size="sm"
            onClick={() => reconnectMutation.mutate()}
            disabled={reconnectMutation.isPending}
          >
            {reconnectMutation.isPending ? "Reconnecting…" : "Save & Reconnect"}
          </Button>
        </SettingRow>
      </SettingGroup>
    </div>
  );
}
