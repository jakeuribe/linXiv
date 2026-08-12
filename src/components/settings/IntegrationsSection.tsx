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
import { errText } from "../../lib/errText";

function IntegrationRow({
  label,
  description,
  installed,
  available = true,
  stale = false,
  configError = false,
  loading,
  error,
  onInstall,
  onUninstall,
}: {
  label: string;
  description: string;
  installed: boolean;
  available?: boolean;
  stale?: boolean;
  configError?: boolean;
  loading: boolean;
  error?: string;
  onInstall: () => void;
  onUninstall: () => void;
}) {
  return (
    <SettingRow
      className={available || installed ? "" : "opacity-45"}
      label={
        <span className="flex items-center gap-2">
          {label}
          {installed && !stale && (
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
          {installed && stale && (
            <span
              className="text-xs px-2 py-0.5 rounded-full font-medium"
              style={{
                background: "color-mix(in srgb, var(--color-danger) 15%, transparent)",
                color: "var(--color-danger)",
              }}
            >
              reinstall needed
            </span>
          )}
          {!installed && configError && (
            <span
              className="text-xs px-2 py-0.5 rounded-full font-medium"
              style={{
                background: "color-mix(in srgb, var(--color-danger) 15%, transparent)",
                color: "var(--color-danger)",
              }}
            >
              config unreadable
            </span>
          )}
          {!available && !configError && (
            <span className="text-xs text-muted">(not detected)</span>
          )}
        </span>
      }
      description={
        <>
          {description}
          {installed && stale && (
            <span className="block">
              The registered linXiv command no longer exists (likely an old install). Reinstall to
              register the current binary.
            </span>
          )}
          {!installed && configError && (
            <span className="block">
              This client's MCP config file exists but isn't valid JSON, so linXiv can't tell
              whether it's registered. Fix or remove the file by hand.
            </span>
          )}
          {error && (
            <span className="block" style={{ color: "var(--color-danger)" }}>
              {error}
            </span>
          )}
        </>
      }
    >
      {loading ? (
        <Spinner size={16} />
      ) : installed ? (
        <>
          {stale && (
            <Button variant="primary" size="sm" onClick={onInstall} disabled={!available}>
              Reinstall
            </Button>
          )}
          <Button variant="danger" size="sm" onClick={onUninstall}>
            Uninstall
          </Button>
        </>
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
  cursor: "Registers linXiv as an MCP server in Cursor (~/.cursor/mcp.json).",
  antigravity: "Registers linXiv as an MCP server in Antigravity (~/.gemini/config/mcp_config.json).",
  windsurf: "Registers linXiv as an MCP server in Windsurf (~/.codeium/windsurf/mcp_config.json).",
  vscode: "Registers linXiv as an MCP server in VS Code's user mcp.json.",
};

export function IntegrationsSection() {
  const qc = useQueryClient();

  const { data: cliInstalled = false, isLoading: cliLoading, isError: cliError, error: cliErrorMsg } = useQuery({
    queryKey: ["cli_installed"],
    queryFn: isCliInstalled,
    staleTime: 10_000,
    enabled: isTauri,
  });

  const { data: mcpClients = [], isLoading: mcpLoading, isError: mcpError, error: mcpErrorMsg } = useQuery({
    queryKey: ["mcp_clients"],
    queryFn: listMcpClients,
    staleTime: 10_000,
    enabled: isTauri,
  });

  const [cliPending, setCliPending] = useState(false);
  const [mcpPending, setMcpPending] = useState<Record<string, boolean>>({});
  // Last install/uninstall error per row key ("cli" or client id).
  const [errors, setErrors] = useState<Record<string, string>>({});

  function setRowError(key: string, message?: string) {
    setErrors((prev) => {
      const next = { ...prev };
      if (message !== undefined) next[key] = message;
      else delete next[key];
      return next;
    });
  }

  async function handleCli(action: "install" | "uninstall") {
    setCliPending(true);
    setRowError("cli");
    try {
      if (action === "install") await installCli();
      else await uninstallCli();
      await qc.invalidateQueries({ queryKey: ["cli_installed"] });
    } catch (e) {
      console.error(e);
      setRowError("cli", errText(e, String(e)));
    } finally {
      setCliPending(false);
    }
  }

  async function handleMcp(clientId: string, action: "install" | "uninstall") {
    setMcpPending((prev) => ({ ...prev, [clientId]: true }));
    setRowError(clientId);
    try {
      if (action === "install") await installMcp(clientId);
      else await uninstallMcp(clientId);
      await qc.invalidateQueries({ queryKey: ["mcp_clients"] });
    } catch (e) {
      console.error(e);
      setRowError(clientId, errText(e, String(e)));
    } finally {
      setMcpPending((prev) => {
        const next = { ...prev };
        delete next[clientId];
        return next;
      });
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
      {cliError ? (
        <p className="py-1 text-sm text-muted" style={{ color: "var(--color-danger)" }}>
          Failed to check if CLI is installed. {cliErrorMsg?.message}
        </p>
      ) : (
        <SettingGroup>
          <IntegrationRow
            label="linxiv CLI"
            description="Adds the `linxiv` command to your terminal PATH."
            installed={cliInstalled}
            available={isTauri}
            loading={cliLoading || cliPending}
            error={errors["cli"]}
            onInstall={() => handleCli("install")}
            onUninstall={() => handleCli("uninstall")}
          />
        </SettingGroup>
      )}

      <SettingGroupLabel className="mt-8">MCP clients</SettingGroupLabel>
      {!isTauri ? (
        <SettingGroup>
          <IntegrationRow
            label="MCP clients"
            description="Register linXiv with Claude Desktop, Claude Code, Cursor, Antigravity, Windsurf, or VS Code."
            installed={false}
            available={false}
            loading={false}
            onInstall={() => {}}
            onUninstall={() => {}}
          />
        </SettingGroup>
      ) : mcpError ? (
        <p className="py-1 text-sm text-muted" style={{ color: "var(--color-danger)" }}>
          Failed to detect MCP clients. {mcpErrorMsg?.message}
        </p>
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
              stale={client.stale}
              configError={client.config_error}
              loading={!!mcpPending[client.id]}
              error={errors[client.id]}
              onInstall={() => handleMcp(client.id, "install")}
              onUninstall={() => handleMcp(client.id, "uninstall")}
            />
          ))}
        </SettingGroup>
      )}
    </div>
  );
}
