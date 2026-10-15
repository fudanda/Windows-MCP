import { useCallback, useEffect, useRef, useState } from "react";
import {
  Activity,
  ArrowUpDown,
  ArrowUpRight,
  Check,
  CheckCircle2,
  ChevronRight,
  Copy,
  Eye,
  EyeOff,
  FileText,
  Globe2,
  KeyRound,
  Languages,
  LoaderCircle,
  Monitor,
  Network,
  Play,
  RefreshCw,
  Save,
  Settings2,
  ShieldCheck,
  Square,
  Terminal,
  Wifi,
  Zap,
} from "lucide-react";
import {
  api,
  connectionChanged,
  connectionConfig,
  native,
  preview,
  preferredConnectionHost,
  validConfig,
  type Config,
  type Snapshot,
} from "./api";
import { errorMessage, translate, type Language } from "./i18n";
import "./App.css";
import TransferPanel from "./TransferPanel";
import { startServices, stopServices, type ServiceStartup } from "./services";

function App() {
  const [snapshot, updateSnapshot] = useState<Snapshot>(preview);
  const setSnapshot = useCallback((data: Snapshot) => {
    updateSnapshot((current) =>
      data.generation < current.generation ? current : data,
    );
  }, []);
  const [draft, setDraft] = useState<Config>(preview.config);
  const [language, setLanguage] = useState<Language>("zh-CN");
  const [loaded, setLoaded] = useState(false);
  const [section, setSection] = useState<
    "overview" | "connection" | "logs" | "transfer"
  >("overview");
  const [busy, setBusy] = useState("");
  const inFlight = useRef(new Set<string>());
  const startup = useRef<ServiceStartup | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [notice, setNotice] = useState<
    "" | "saved" | "copied" | "connected" | "servicesStarted" | "servicesStopped"
  >("");
  const [showToken, setShowToken] = useState(false);
  const [format, setFormat] = useState<"toml" | "json">("toml");
  const [host, setHost] = useState("127.0.0.1");
  const defaultHost = preferredConnectionHost(snapshot.addresses, draft.lanEnabled);
  const tr = (key: Parameters<typeof translate>[1]) => translate(language, key);
  const active = ["running", "starting", "stopping"].includes(snapshot.phase);
  const dirty = connectionChanged(draft, snapshot.config);
  const valid = validConfig(draft);
  const load = useCallback(async () => {
    try {
      const data = await api.snapshot();
      setSnapshot(data);
      setDraft(data.config);
      setHost(preferredConnectionHost(data.addresses, data.config.lanEnabled));
      setLanguage(data.config.language);
      setLoaded(true);
      setError(null);
    } catch (error) {
      setError(error);
    }
  }, [setSnapshot]);
  useEffect(() => {
    void load();
  }, [load]);
  useEffect(() => {
    if (!native || !loaded) return;
    let disposed = false;
    const timer = setInterval(() => {
      void api
        .snapshot()
        .then((data) => {
          if (!disposed) setSnapshot(data);
        })
        .catch((error) => {
          if (!disposed) setError(error);
        });
    }, 1500);
    return () => {
      disposed = true;
      clearInterval(timer);
    };
  }, [loaded, setSnapshot]);
  useEffect(() => {
    document.documentElement.lang = language;
    document.title = translate(language, "app");
  }, [language]);
  useEffect(() => {
    setHost(defaultHost);
  }, [defaultHost]);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(""), 3000);
    return () => clearTimeout(timer);
  }, [notice]);
  async function action(
    name: string,
    callback: (signal?: AbortSignal) => Promise<void>,
  ) {
    const stopping = name === "stop" || name === "stopAll";
    if (
      inFlight.current.has(name) ||
      inFlight.current.has("stop") ||
      inFlight.current.has("stopAll") ||
      (!stopping && inFlight.current.size > 0)
    ) return;
    inFlight.current.add(name);
    setBusy(name);
    setError(null);
    setNotice("");
    const controller = ["start", "startAll"].includes(name)
      ? new AbortController()
      : null;
    try {
      const promise = callback(controller?.signal);
      if (controller) startup.current = { controller, promise };
      await promise;
    } catch (error) {
      if (!controller?.signal.aborted) setError(error);
    } finally {
      if (controller && startup.current?.controller === controller)
        startup.current = null;
      inFlight.current.delete(name);
      const remaining = Array.from(inFlight.current);
      setBusy(remaining[remaining.length - 1] ?? "");
    }
  }
  async function changeLanguage(next: Language) {
    const previous = language;
    setLanguage(next);
    setDraft((current) => ({ ...current, language: next }));
    if (native)
      try {
        setSnapshot(await api.language(next));
      } catch (error) {
        setLanguage(previous);
        setDraft((current) => ({ ...current, language: previous }));
        setError(error);
      }
  }
  async function copy(text: string) {
    await action("copy", async () => {
      await api.copy(text);
      setNotice("copied");
    });
  }
  const displayError = error ?? snapshot.error;
  const nav = [
    { id: "overview", icon: Settings2 },
    { id: "connection", icon: Network },
    { id: "transfer", icon: ArrowUpDown },
    { id: "logs", icon: Terminal },
  ] as const;
  const connection = (
    <section className="card connection-card">
      <div className="card-heading">
        <span className="icon-box">
          <Network size={19} />
        </span>
        <div>
          <h2>{tr("connectionTitle")}</h2>
          <p>{tr("connectionHint")}</p>
        </div>
      </div>
      <label className="field-label" htmlFor="connect-host">
        {tr("adapter")}
      </label>
      <select
        id="connect-host"
        value={host}
        onChange={(event) => setHost(event.target.value)}
      >
        <option value="127.0.0.1">127.0.0.1 · {tr("local")}</option>
        {draft.lanEnabled &&
          snapshot.addresses.map((address) => (
            <option key={address.ip} value={address.ip}>
              {address.ip} · {address.name}
            </option>
          ))}
      </select>
      <div className="endpoint">
        <code>{`http://${host}:${draft.port || 8001}/mcp`}</code>
        <button
          className="icon-button"
          aria-label={tr("copyAddress")}
          onClick={() => void copy(`http://${host}:${draft.port}/mcp`)}
          disabled={!valid}
        >
          <Copy size={16} />
        </button>
      </div>
      <div className="code-toolbar">
        <div className="segmented">
          {(["toml", "json"] as const).map((value) => (
            <button
              key={value}
              className={format === value ? "selected" : ""}
              onClick={() => setFormat(value)}
            >
              {value.toUpperCase()}
            </button>
          ))}
        </div>
        <span>HTTP MCP</span>
      </div>
      <pre className="config-code">
        <code>{connectionConfig(draft, host, format, true)}</code>
      </pre>
      <p className="field-hint">{tr("configHint")}</p>
      <button
        className="button secondary full"
        onClick={() => void copy(connectionConfig(draft, host, format))}
        disabled={!native || !valid || Boolean(busy)}
      >
        <Copy size={16} />
        {tr("copyConfig")}
      </button>
      {snapshot.phase !== "running" && (
        <div className="inline-note">
          <Activity size={15} />
          <span>{tr("pendingHint")}</span>
        </div>
      )}
      {draft.lanEnabled && (
        <p className="field-hint">
          {snapshot.addresses.length ? tr("lanNote") : tr("noLan")}
        </p>
      )}
    </section>
  );
  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-symbol">
            <Monitor size={23} />
          </span>
          <div>
            <strong>Windows-MCP</strong>
            <span>Manager</span>
          </div>
        </div>
        <div className="workspace-label">{tr("official")}</div>
        <nav aria-label={tr("app")}>
          {nav.map((item) => (
            <button
              key={item.id}
              onClick={() => setSection(item.id)}
              className={section === item.id ? "nav-item current" : "nav-item"}
            >
              <item.icon size={18} />
              <span>{tr(item.id)}</span>
              {section === item.id && <ChevronRight size={14} />}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <div className="backend-label">
            <ShieldCheck size={17} />
            <span>CursorTouch / Windows-MCP</span>
          </div>
          <p>{tr("runtime")}</p>
          <span className="version-label">Tauri 2 · v0.2.4</span>
        </div>
      </aside>
      <div className="main-shell">
        <header className="topbar">
          <div className="breadcrumb">
            Windows-MCP <ChevronRight size={14} />
            <span>{tr(section)}</span>
          </div>
          <label className="language-control">
            <Languages size={16} />
            <select
              aria-label={tr("language")}
              value={language}
              onChange={(event) =>
                void changeLanguage(event.target.value as Language)
              }
            >
              <option value="zh-CN">简体中文</option>
              <option value="en">English</option>
            </select>
          </label>
        </header>
        <main>
          <div className="page-heading">
            <div>
              <div className="eyebrow">WINDOWS-MCP MANAGER</div>
              <h1>{tr(section)}</h1>
              <p>{tr("subtitle")}</p>
            </div>
            <span className="official-badge">
              <ShieldCheck size={15} />
              {tr("official")}
            </span>
          </div>
          {!native && (
            <div className="preview-banner">
              <Monitor size={16} />
              {tr("preview")}
            </div>
          )}
          {Boolean(displayError) && (
            <div className="error-banner" role="alert">
              <span>{errorMessage(language, displayError)}</span>
              {typeof displayError === "object" &&
                displayError !== null &&
                "detail" in displayError &&
                Boolean(displayError.detail) && (
                  <details>
                    <summary>{language === "en" ? "Details" : "详情"}</summary>
                    <code>{String(displayError.detail)}</code>
                  </details>
                )}
            </div>
          )}
          {notice && (
            <div className="toast" role="status">
              <Check size={16} />
              {tr(notice)}
            </div>
          )}
          {!loaded ? (
            <div className="loading">
              <LoaderCircle className="spin" size={22} />
              {tr("loading")}
              {error !== null && (
                <button
                  className="button secondary"
                  onClick={() => void load()}
                >
                  {tr("retry")}
                </button>
              )}
            </div>
          ) : (
            <>
              {section === "transfer" && (
                <TransferPanel
                  language={language}
                  addresses={snapshot.addresses}
                />
              )}
              {section === "overview" && (
                <>
                  <section
                    className={`service-card ${snapshot.phase === "running" ? "online" : ""}`}
                  >
                    <div className="service-top">
                      <div className="service-identity">
                        <span className="service-icon">
                          <Monitor size={27} />
                        </span>
                        <div>
                          <h2>{tr("service")}</h2>
                          <p>{tr("serviceHint")}</p>
                        </div>
                      </div>
                      <span className={`status-badge ${snapshot.phase}`}>
                        <span />
                        {tr(snapshot.phase)}
                      </span>
                    </div>
                    <div className="service-actions">
                      <button
                        className="button primary"
                        disabled={
                          !native ||
                          !snapshot.backendAvailable ||
                          ["starting", "stopping"].includes(snapshot.phase) ||
                          Boolean(busy) ||
                          !valid
                        }
                        onClick={() =>
                          void action("startAll", async (signal) => {
                            await startServices(
                              snapshot,
                              { ...draft, language },
                              (data) => {
                                setSnapshot(data);
                                setDraft(data.config);
                              },
                              signal,
                            );
                            setNotice("servicesStarted");
                          })
                        }
                      >
                        {busy === "startAll" ? (
                          <LoaderCircle size={17} className="spin" />
                        ) : (
                          <Zap size={17} />
                        )}
                        {busy === "startAll" ? tr("startingAll") : tr("startAll")}
                      </button>
                      <button
                        className="button secondary"
                        title={tr("stopAllHint")}
                        disabled={
                          !native ||
                          (Boolean(busy) && !["start", "startAll"].includes(busy))
                        }
                        onClick={() =>
                          void action("stopAll", async () => {
                            await stopServices(language, setSnapshot, startup.current);
                            setNotice("servicesStopped");
                          })
                        }
                      >
                        {busy === "stopAll" ? (
                          <LoaderCircle size={17} className="spin" />
                        ) : (
                          <Square size={14} />
                        )}
                        {busy === "stopAll" ? tr("stoppingAll") : tr("stopAll")}
                      </button>
                      <button
                        className="button secondary"
                        disabled={
                          !native ||
                          !snapshot.backendAvailable ||
                          active ||
                          Boolean(busy) ||
                          !valid
                        }
                        onClick={() =>
                          void action("start", async (signal) => {
                            let generation = snapshot.generation;
                            if (dirty) {
                              const saved = await api.save({
                                ...draft,
                                language,
                              });
                              setSnapshot(saved);
                              generation = saved.generation;
                            }
                            signal?.throwIfAborted();
                            setSnapshot(await api.start(generation));
                          })
                        }
                      >
                        {busy === "start" || snapshot.phase === "starting" ? (
                          <LoaderCircle size={17} className="spin" />
                        ) : (
                          <Play size={17} />
                        )}{" "}
                        {dirty ? tr("saveStart") : tr("start")}
                      </button>
                      <button
                        className="button secondary"
                        disabled={
                          !native ||
                          !["starting", "running"].includes(snapshot.phase) ||
                          busy === "stop" ||
                          busy === "stopAll"
                        }
                        onClick={() =>
                          void action("stop", async () => {
                            setSnapshot(await api.stop());
                          })
                        }
                      >
                        <Square size={14} />
                        {tr("stop")}
                      </button>
                      <button
                        className="text-button"
                        disabled={snapshot.phase !== "running" || Boolean(busy)}
                        onClick={() =>
                          void action("test", async () => {
                            await api.test();
                            setSnapshot(await api.snapshot());
                            setNotice("connected");
                          })
                        }
                      >
                        <RefreshCw
                          size={15}
                          className={busy === "test" ? "spin" : ""}
                        />
                        {busy === "test" ? tr("testing") : tr("test")}
                      </button>
                    </div>
                    <div className="service-facts">
                      <div>
                        <span>{tr("access")}</span>
                        <strong>
                          {snapshot.config.lanEnabled ? tr("lan") : tr("local")}
                        </strong>
                      </div>
                      <div>
                        <span>{tr("tools")}</span>
                        <strong>
                          {snapshot.probe?.toolCount ?? tr("empty")}
                        </strong>
                      </div>
                      <div>
                        <span>{tr("processId")}</span>
                        <strong>{snapshot.pid ?? tr("empty")}</strong>
                      </div>
                      <div>
                        <span>{tr("activeSince")}</span>
                        <strong>
                          {snapshot.startedAt
                            ? new Date(
                                snapshot.startedAt * 1000,
                              ).toLocaleTimeString(language)
                            : tr("empty")}
                        </strong>
                      </div>
                    </div>
                  </section>
                  <div className="content-grid">
                    <section className="card settings-card">
                      <div className="card-heading">
                        <span className="icon-box">
                          <Settings2 size={19} />
                        </span>
                        <div>
                          <h2>{tr("settings")}</h2>
                          <p>
                            {active ? tr("stopToEdit") : tr("settingsHint")}
                          </p>
                        </div>
                      </div>
                      <fieldset disabled={Boolean(busy)}>
                        <label className="field-label" htmlFor="port">
                          {tr("port")}
                        </label>
                        <input
                          id="port"
                          disabled={active}
                          inputMode="numeric"
                          type="number"
                          min="1"
                          max="65535"
                          value={draft.port || ""}
                          onChange={(event) =>
                            setDraft({
                              ...draft,
                              port: Number(event.target.value),
                            })
                          }
                        />
                        <p className="field-hint">{tr("portHint")}</p>
                        <div className="network-option">
                          <div className="network-copy">
                            <Wifi size={18} />
                            <label htmlFor="lan">
                              <strong>{tr("lanTitle")}</strong>
                              <span>
                                {draft.lanEnabled
                                  ? tr("lanHint")
                                  : tr("localHint")}
                              </span>
                            </label>
                          </div>
                          <input
                            id="lan"
                            disabled={active}
                            type="checkbox"
                            role="switch"
                            checked={draft.lanEnabled}
                            onChange={(event) => {
                              const enabled = event.target.checked;
                              setDraft({ ...draft, lanEnabled: enabled });
                            }}
                          />
                        </div>
                        <div className="token-label">
                          <label className="field-label" htmlFor="token">
                            <KeyRound size={14} />
                            {tr("token")}
                          </label>
                          <button
                            className="text-button"
                            disabled={!native || active}
                            onClick={() =>
                              void action("generate", async () => {
                                const token = await api.generateToken();
                                setDraft((current) => ({ ...current, token }));
                              })
                            }
                          >
                            <RefreshCw size={13} />
                            {tr("generate")}
                          </button>
                        </div>
                        <div className="token-input">
                          <input
                            id="token"
                            readOnly={active}
                            autoComplete="off"
                            spellCheck={false}
                            type={showToken ? "text" : "password"}
                            value={draft.token}
                            placeholder={
                              native ? "" : "••••••••••••••••••••••••••••••••"
                            }
                            onChange={(event) =>
                              setDraft({ ...draft, token: event.target.value })
                            }
                          />
                          <button
                            className="icon-button"
                            aria-label={showToken ? tr("hide") : tr("show")}
                            onClick={() => setShowToken(!showToken)}
                          >
                            {showToken ? (
                              <EyeOff size={17} />
                            ) : (
                              <Eye size={17} />
                            )}
                          </button>
                          <button
                            className="icon-button"
                            disabled={!native || !draft.token}
                            aria-label={tr("copyToken")}
                            onClick={() => void copy(draft.token)}
                          >
                            <Copy size={15} />
                          </button>
                        </div>
                        <p className="field-hint">{tr("tokenHint")}</p>
                      </fieldset>
                      <div className="settings-footer">
                        <span className={dirty ? "unsaved" : "saved"}>
                          {dirty ? (
                            <span className="unsaved-dot" />
                          ) : (
                            <CheckCircle2 size={14} />
                          )}{" "}
                          {dirty ? tr("dirty") : tr("saved")}
                        </span>
                        <button
                          className="button primary small"
                          disabled={
                            !native ||
                            active ||
                            Boolean(busy) ||
                            !dirty ||
                            !valid
                          }
                          onClick={() =>
                            void action("save", async () => {
                              const data = await api.save({
                                ...draft,
                                language,
                              });
                              setSnapshot(data);
                              setDraft(data.config);
                              setNotice("saved");
                            })
                          }
                        >
                          <Save size={15} />
                          {tr("save")}
                        </button>
                      </div>
                    </section>
                    {connection}
                  </div>
                </>
              )}
              {section === "connection" && (
                <div className="connection-page">
                  {connection}
                  <section className="card guide-card">
                    <span className="guide-icon">
                      <Globe2 size={28} />
                    </span>
                    <h2>{tr("protocol")}</h2>
                    <p>{tr("connectionHint")}</p>
                    <div className="guide-point">
                      <ShieldCheck size={18} />
                      {tr("auth")}
                    </div>
                    <div className="guide-point">
                      <KeyRound size={18} />
                      {tr("encrypted")}
                    </div>
                    <p>{draft.lanEnabled ? tr("lanNote") : tr("localHint")}</p>
                  </section>
                </div>
              )}
              {section === "logs" && (
                <section className="card">
                  <div className="card-heading">
                    <span className="icon-box">
                      <FileText size={19} />
                    </span>
                    <div>
                      <h2>{tr("logs")}</h2>
                      <p>{tr("logHint")}</p>
                    </div>
                  </div>
                  <div className="logs-console" role="log">
                    {snapshot.logs.length ? (
                      snapshot.logs.map((line, index) => (
                        <div
                          className="log-line"
                          key={`${index}-${line.timestamp}`}
                        >
                          <time>
                            {new Date(line.timestamp * 1000).toLocaleTimeString(
                              language,
                            )}
                          </time>
                          <span>{line.message}</span>
                        </div>
                      ))
                    ) : (
                      <div className="logs-empty">
                        <Terminal size={28} />
                        <span>{tr("noLogs")}</span>
                      </div>
                    )}
                  </div>
                </section>
              )}
              <footer>
                <span>
                  <ShieldCheck size={14} />
                  {tr("closeHint")}
                </span>
                <span>
                  Windows-MCP <ArrowUpRight size={12} />
                </span>
              </footer>
              {snapshot.configPath && (
                <p className="config-path">
                  {tr("configLocation")}：{snapshot.configPath}
                </p>
              )}
              {native && !snapshot.backendAvailable && (
                <p className="backend-missing">{tr("backendMissing")}</p>
              )}
            </>
          )}
        </main>
      </div>
    </div>
  );
}
export default App;
