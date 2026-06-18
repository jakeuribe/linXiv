import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  isCliInstalled,
  installCli,
  uninstallCli,
  listMcpClients,
  installMcp,
  uninstallMcp,
  type MpcClientStatus,
} from "../../api/integrations";
import { isTauri } from "../../api/client";
import { Button } from "../ui/button";
import { Spinner } from "../ui/spinner";
import { SettingGroup, SettingGroupLabel, SettingRow } from "./SettingRow";

function IntegrationRow({
  label,
  description,
  installed,
  available = true,
  loading,
  onInstall,
  onUninstall,
}: {
  label: string;
  description: string;
  installed: boolean;
  available?: boolean;
  loading: boolean;
  onInstall: () => void;
  onUninstall: () => void;
}) {
  return (
    <SettingRow
      className={available ? "" : "opacity-45"}
      label={
        <span className="flex items-center gap-2">
          {label}
          {installed && (
            <span
              className="text-xs px-2 py-0.5 rounded-full font-medium"
              style={{
                background: "color-mix(in srgb, var(--color-success) 15%, transparent)",
                color: "var(--color-success)",
              }}
            >
              installed
            </span>
          )}
          {!available && <span className="text-xs text-muted">(not detected)</span>}
        </span>
      }
      description={description}
    >
      {loading ? (
        <Spinner size={16} />
      ) : installed ? (
        <Button variant="danger" size="sm" onClick={onUninstall}>
          Uninstall
        </Button>
      ) : (
        <Button variant="primary" size="sm" onClick={onInstall} disabled={!available}>
          Install
        </Button>
      )}
    </SettingRow>
  );
}

const mcpClientDescriptions: Record<string, string> = {
  claude: "Registers linXiv as an MCP server in Claude Desktop.",
  "claude-code": "Registers linXiv as an MCP server in Claude Code CLI (~/.claude.json).",
  cursor: "Registers linXiv as an MCP server in Cursor.",
  antigravity: "Registers linXiv as an MCP server in Antigravity.",
};

export function IntegrationsSection() {
  const qc = useQueryClient();

  const { data: cliInstalled = false, isLoading: cliLoading } = useQuery({
    queryKey: ["cli_installed"],
    queryFn: isCliInstalled,
    staleTime: 10_000,
    enabled: isTauri,
  });

  const { data: mcpClients = [], isLoading: mcpLoading } = useQuery({
    queryKey: ["mcp_clients"],
    queryFn: listMcpClients,
    staleTime: 10_000,
    enabled: isTauri,
  });

  const [cliPending, setCliPending] = useState(false);
  const [mcpPending, setMcpPending] = useState<string | null>(null);

  async function handleCli(action: "install" | "uninstall") {
    setCliPending(true);
    try {
      if (action === "install") await installCli();
      else await uninstallCli();
      await qc.invalidateQueries({ queryKey: ["cli_installed"] });
    } catch (e) {
      console.error(e);
    } finally {
      setCliPending(false);
    }
  }

  async function handleMcp(clientId: string, action: "install" | "uninstall") {
    setMcpPending(clientId);
    try {
      if (action === "install") await installMcp(clientId);
      else await uninstallMcp(clientId);
      await qc.invalidateQueries({ queryKey: ["mcp_clients"] });
    } catch (e) {
      console.error(e);
    } finally {
      setMcpPending(null);
    }
  }

  return (
    <div>
      <SettingGroupLabel>Integrations</SettingGroupLabel>
      <p className="mb-2.5 text-xs text-muted">
        Install linXiv tools so other apps can use them outside the GUI.
      </p>
      {!isTauri && (
        <p className="mb-2.5 text-xs text-muted italic">
          Available in the desktop app. The browser dev build can't install
          system-level integrations.
        </p>
      )}

      <SettingGroupLabel className="mt-8">Command line</SettingGroupLabel>
      <SettingGroup>
        <IntegrationRow
          label="linxiv CLI"
          description="Adds the `linxiv` command to your terminal PATH."
          installed={cliInstalled}
          available={isTauri}
          loading={cliLoading || cliPending}
          onInstall={() => handleCli("install")}
          onUninstall={() => handleCli("uninstall")}
        />
      </SettingGroup>

      <SettingGroupLabel className="mt-8">MCP clients</SettingGroupLabel>
      {!isTauri ? (
        <SettingGroup>
          <IntegrationRow
            label="MCP clients"
            description="Register linXiv with Claude Desktop, Claude Code, Cursor, or Antigravity."
            installed={false}
            available={false}
            loading={false}
            onInstall={() => {}}
            onUninstall={() => {}}
          />
        </SettingGroup>
      ) : mcpLoading ? (
        <p className="flex items-center gap-2 py-1 text-sm text-muted">
          <Spinner size={14} /> Detecting clients…
        </p>
      ) : mcpClients.length === 0 ? (
        <p className="py-1 text-sm text-muted">No MCP clients detected.</p>
      ) : (
        <SettingGroup>
          {mcpClients.map((client: MpcClientStatus) => (
            <IntegrationRow
              key={client.id}
              label={client.name}
              description={mcpClientDescriptions[client.id] ?? `Registers linXiv as an MCP server in ${client.name}.`}
              installed={client.installed}
              available={client.available}
              loading={mcpPending === client.id}
              onInstall={() => handleMcp(client.id, "install")}
              onUninstall={() => handleMcp(client.id, "uninstall")}
            />
          ))}
        </SettingGroup>
      )}
    </div>
  );
}
