import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowUpDown,
  Copy,
  FolderOpen,
  Globe2,
  KeyRound,
  Play,
  Save,
  Square,
} from "lucide-react";
import { api, native, preferredConnectionHost } from "./api";
import type { Language } from "./i18n";
import {
  transferApi,
  type TransferSettings,
  type TransferSnapshot,
} from "./transfer-api";
import { transferError, transferText } from "./transfer-i18n";
import { TransferTasks } from "./TransferTasks";
import "./transfer.css";

export default function TransferPanel({
  language,
  addresses,
}: {
  language: Language;
  addresses: { ip: string; name: string }[];
}) {
  const t = (key: Parameters<typeof transferText>[1]) =>
    transferText(language, key);
  const [state, setState] = useState<TransferSnapshot | null>(null),
    [draft, setDraft] = useState<TransferSettings | null>(null);
  const savedSettings = useRef<TransferSettings | null>(null);
  const [busy, setBusy] = useState(false),
    [error, setError] = useState<unknown>(null),
    [notice, setNotice] = useState("");
  const defaultHost = preferredConnectionHost(addresses, draft?.lanEnabled ?? true);
  const [host, setHost] = useState(defaultHost);
  useEffect(() => setHost(defaultHost), [defaultHost]);
  const [peer, setPeer] = useState({ name: "", url: "", token: "" }),
    [peerId, setPeerId] = useState(""),
    [paths, setPaths] = useState("");
  useEffect(() => {
    if (!native) return;
    let disposed = false;
    const refresh = async () => {
      try {
        const data = await transferApi.snapshot();
        if (!disposed) {
          setState(data);
          const previous = savedSettings.current;
          setDraft((old) =>
            !old ||
            (previous &&
              old.port === previous.port &&
              old.sharedDir === previous.sharedDir &&
              old.lanEnabled === previous.lanEnabled &&
              old.token === previous.token)
              ? data.settings
              : old,
          );
          savedSettings.current = data.settings;
        }
      } catch (e) {
        if (!disposed) setError(e);
      }
    };
    void refresh();
    const timer = setInterval(() => void refresh(), 1000);
    return () => {
      disposed = true;
      clearInterval(timer);
    };
  }, []);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(""), 2500);
    return () => clearTimeout(timer);
  }, [notice]);
  async function action(work: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await work();
    } catch (e) {
      setError(e);
    } finally {
      setBusy(false);
    }
  }
  async function copy(text: string) {
    await api.copy(text);
    setNotice(t("copied"));
  }
  async function save() {
    if (!draft) return;
    const data = await transferApi.command("configure", {
      config: {
        port: draft.port,
        lanEnabled: draft.lanEnabled,
        sharedDir: draft.sharedDir,
        token: draft.token,
      },
    });
    setState(data);
    setDraft(data.settings);
    setNotice(t("saved"));
  }
  async function choose(directory: boolean) {
    const chosen = await open({ directory, multiple: !directory });
    if (chosen) setPaths(Array.isArray(chosen) ? chosen.join("\n") : chosen);
  }
  if (!native) return <div className="preview-banner">{t("preview")}</div>;
  if (!state || !draft)
    return (
      <div className="loading">
        {error ? transferError(language, error) : t("preparing")}
      </div>
    );
  const active = state.tasks.some(
      (task) => !["completed", "failed", "cancelled"].includes(task.status),
    ),
    locked = state.running || active || busy;
  const address = `http://${draft.lanEnabled ? host : "127.0.0.1"}:${draft.port}`;
  return (
    <div className="transfer-panel">
      {error !== null && (
        <div className="error-banner" role="alert">
          {transferError(language, error)}
          {typeof error === "object" && error !== null && "detail" in error && (
            <details>
              <summary>{language === "en" ? "Details" : "详情"}</summary>
              <code>{String(error.detail)}</code>
            </details>
          )}
        </div>
      )}
      {notice && (
        <div className="toast" role="status">
          {notice}
        </div>
      )}
      <section className={`service-card ${state.running ? "online" : ""}`}>
        <div className="service-top">
          <div className="service-identity">
            <span className="service-icon">
              <ArrowUpDown size={27} />
            </span>
            <div>
              <h2>{t("service")}</h2>
              <p>{t("noClient")}</p>
            </div>
          </div>
          <span
            className={`status-badge ${state.running ? "running" : "stopped"}`}
          >
            <span />
            {state.running ? t("ready") : t("stopped")}
          </span>
        </div>
        <div className="service-actions">
          <button
            className="button primary"
            disabled={busy || state.running}
            onClick={() =>
              void action(async () => {
                await save();
                setState(await transferApi.command("start"));
              })
            }
          >
            <Play size={16} />
            {t("start")}
          </button>
          <button
            className="button secondary"
            disabled={busy || (!state.running && !active)}
            onClick={() =>
              void action(async () =>
                setState(await transferApi.command("stop")),
              )
            }
          >
            <Square size={14} />
            {t("stop")}
          </button>
          <small>{state.engine}</small>
        </div>
      </section>
      <div className="transfer-settings-grid">
        <section className="card">
          <div className="card-heading">
            <span className="icon-box">
              <FolderOpen size={19} />
            </span>
            <div>
              <h2>{t("directory")}</h2>
              <p>{locked ? t("stopHint") : t("noOverwrite")}</p>
            </div>
          </div>
          <fieldset disabled={locked}>
            <label className="field-label" htmlFor="transfer-root">
              {t("directory")}
            </label>
            <div className="transfer-input-row">
              <input
                id="transfer-root"
                value={draft.sharedDir}
                onChange={(e) =>
                  setDraft({ ...draft, sharedDir: e.target.value })
                }
              />
              <button
                className="button secondary"
                onClick={() =>
                  void action(async () => {
                    const folder = await open({
                      directory: true,
                      multiple: false,
                    });
                    if (typeof folder === "string")
                      setDraft({ ...draft, sharedDir: folder });
                  })
                }
              >
                <FolderOpen size={16} />
                {t("chooseDirectory")}
              </button>
            </div>
            <label className="field-label" htmlFor="transfer-port">
              {t("port")}
            </label>
            <input
              id="transfer-port"
              type="number"
              min={1}
              max={65535}
              value={draft.port || ""}
              onChange={(e) =>
                setDraft({ ...draft, port: Number(e.target.value) })
              }
            />
            <label className="transfer-checkbox">
              <input
                id="transfer-lan"
                type="checkbox"
                checked={draft.lanEnabled}
                onChange={(e) =>
                  setDraft({ ...draft, lanEnabled: e.target.checked })
                }
              />
              {t("lan")}
            </label>
            <label className="field-label" htmlFor="transfer-token">
              {t("token")}
            </label>
            <div className="transfer-input-row">
              <input
                id="transfer-token"
                type="password"
                autoComplete="off"
                value={draft.token}
                onChange={(e) => setDraft({ ...draft, token: e.target.value })}
              />
              <button
                className="button secondary"
                onClick={() =>
                  void action(async () =>
                    setDraft({ ...draft, token: await api.generateToken() }),
                  )
                }
              >
                <KeyRound size={16} />
                {t("generate")}
              </button>
            </div>
            <p className="field-hint">{t("tokenHint")}</p>
            <button
              className="button secondary"
              onClick={() => void action(save)}
            >
              <Save size={16} />
              {t("save")}
            </button>
          </fieldset>
        </section>
        <section className="card">
          <div className="card-heading">
            <span className="icon-box">
              <Globe2 size={19} />
            </span>
            <div>
              <h2>{t("address")}</h2>
              <p>{t("noClient")}</p>
            </div>
          </div>
          <select
            aria-label={t("address")}
            value={host}
            onChange={(e) => setHost(e.target.value)}
          >
            <option value="127.0.0.1">127.0.0.1</option>
            {addresses.map((a) => (
              <option value={a.ip} key={a.ip}>
                {a.ip} · {a.name}
              </option>
            ))}
          </select>
          <div className="endpoint">
            <code>{address}</code>
          </div>
          <div className="transfer-button-row">
            <button
              className="button secondary"
              onClick={() => void action(() => copy(address))}
              disabled={busy}
            >
              <Copy size={16} />
              {t("copyAddress")}
            </button>
            <button
              className="button secondary"
              onClick={() => void action(() => copy(draft.token))}
              disabled={busy}
            >
              <KeyRound size={16} />
              {t("copyToken")}
            </button>
          </div>
          <p className="transfer-instructions">{t("browserHint")}</p>
          <p className="field-hint">{t("library")}</p>
        </section>
      </div>
      <TransferTasks
        language={language}
        tasks={state.tasks}
        cancel={(id) =>
          void action(async () =>
            setState(await transferApi.command("cancel", { taskId: id })),
          )
        }
      />
      <details className="card transfer-advanced">
        <summary>{t("advanced")}</summary>
        <div className="transfer-settings-grid">
          <div>
            <label className="field-label" htmlFor="peer-name">
              {t("peerName")}
            </label>
            <input
              id="peer-name"
              value={peer.name}
              onChange={(e) => setPeer({ ...peer, name: e.target.value })}
            />
            <label className="field-label" htmlFor="peer-url">
              {t("peerUrl")}
            </label>
            <input
              id="peer-url"
              placeholder="http://192.168.1.10:8002"
              value={peer.url}
              onChange={(e) => setPeer({ ...peer, url: e.target.value })}
            />
            <label className="field-label" htmlFor="peer-token">
              {t("peerToken")}
            </label>
            <input
              id="peer-token"
              type="password"
              autoComplete="off"
              value={peer.token}
              onChange={(e) => setPeer({ ...peer, token: e.target.value })}
            />
            <button
              className="button secondary"
              disabled={busy || !peer.name || !peer.url || !peer.token}
              onClick={() =>
                void action(async () => {
                  const data = await transferApi.command("add_peer", { peer });
                  setState(data);
                  setPeerId(
                    data.settings.peers.find(
                      (p) => p.url === peer.url.replace(/\/$/, ""),
                    )?.id ?? "",
                  );
                  setPeer({ name: "", url: "", token: "" });
                })
              }
            >
              {t("addPeer")}
            </button>
          </div>
          <div>
            <label className="field-label" htmlFor="peer-select">
              {t("selectPeer")}
            </label>
            <select
              id="peer-select"
              value={peerId}
              onChange={(e) => setPeerId(e.target.value)}
            >
              <option value="">{t("selectPeer")}</option>
              {state.settings.peers.map((p) => (
                <option value={p.id} key={p.id}>
                  {p.name} · {p.url}
                </option>
              ))}
            </select>
            <button
              className="button secondary"
              disabled={busy || !peerId}
              onClick={() =>
                void action(async () => {
                  setState(
                    await transferApi.command("remove_peer", { peerId }),
                  );
                  setPeerId("");
                })
              }
            >
              {t("removePeer")}
            </button>
            <label className="field-label" htmlFor="transfer-paths">
              {t("paths")}
            </label>
            <textarea
              id="transfer-paths"
              rows={4}
              value={paths}
              onChange={(e) => setPaths(e.target.value)}
            />
            <div className="transfer-button-row">
              <button
                className="button secondary"
                onClick={() => void action(() => choose(false))}
                disabled={busy}
              >
                {t("chooseFiles")}
              </button>
              <button
                className="button secondary"
                onClick={() => void action(() => choose(true))}
                disabled={busy}
              >
                {t("chooseFolder")}
              </button>
              <button
                className="button primary"
                disabled={busy || !peerId || !paths.trim()}
                onClick={() =>
                  void action(async () => {
                    await transferApi.command("send", {
                      peerId,
                      paths: paths
                        .split(/\r?\n/)
                        .map((p) => p.trim())
                        .filter(Boolean),
                    });
                    setState(await transferApi.snapshot());
                  })
                }
              >
                <ArrowUpDown size={16} />
                {t("sendFiles")}
              </button>
            </div>
          </div>
        </div>
      </details>
    </div>
  );
}
