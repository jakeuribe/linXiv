import { useNavigate } from "react-router";
import { sharingAvailable } from "../../api/share";
import { Button } from "../ui/button";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

// Ticket generation, join-by-ticket, and the received-shares list now live
// on the SharePage (/shared) instead of being duplicated here. This section
// is the future home for device/relay-level sharing settings (e.g. pointing
// at a custom iroh relay) once those exist.
export function SharingSection() {
  const navigate = useNavigate();

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
      </SettingGroup>
    </div>
  );
}
