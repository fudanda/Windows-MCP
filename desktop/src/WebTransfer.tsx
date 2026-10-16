import { useCallback, useEffect, useRef, useState } from "react";
import appIcon from "../src-tauri/icons/128x128.png";
import Uppy from "@uppy/core";
import Dashboard from "@uppy/dashboard";
import XHRUpload from "@uppy/xhr-upload";
import zhCN from "@uppy/locales/lib/zh_CN.js";
import enUS from "@uppy/locales/lib/en_US.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex } from "@noble/hashes/utils.js";
import {
  ArrowUp,
  ArrowUpDown,
  Download,
  File,
  Folder,
  Languages,
  LogOut,
  RefreshCw,
  ShieldCheck,
} from "lucide-react";
import { fileUrl, webRequest, type TransferTask } from "./transfer-api";
import { bytes, transferError, transferText } from "./transfer-i18n";
import type { Language } from "./i18n";
import { TransferTasks } from "./TransferTasks";
import "@uppy/core/css/style.min.css";
import "@uppy/dashboard/css/style.min.css";
import "./App.css";
import "./transfer.css";

interface ListedFile {
  name: string;
  path_type: string;
  size: number;
}
export default function WebTransfer() {
  const [language, setLanguage] = useState<Language>(() =>
    localStorage.getItem("windowsMcpTransferLanguage") === "en"
      ? "en"
      : "zh-CN",
  );
  const languageRef = useRef(language);
  languageRef.current = language;
  const t = (key: Parameters<typeof transferText>[1]) =>
    transferText(language, key);
  const [connected, setConnected] = useState(false),
    [token, setToken] = useState(""),
    [busy, setBusy] = useState(false),
    [error, setError] = useState<unknown>(null);
  const [hostName, setHostName] = useState("Windows"),
    [path, setPath] = useState(""),
    [files, setFiles] = useState<ListedFile[]>([]),
    [tasks, setTasks] = useState<TransferTask[]>([]),
    [checksum, setChecksum] = useState("");
  const dashboard = useRef<HTMLDivElement>(null),
    uppyRef = useRef<Uppy | null>(null),
    folderInput = useRef<HTMLInputElement>(null),
    fileInput = useRef<HTMLInputElement>(null);
  const pathRef = useRef(path);
  pathRef.current = path;
  const refresh = useCallback(async () => {
    try {
      const listing = await webRequest<{ paths: ListedFile[] }>(
        fileUrl(path, "?json"),
      );
      setFiles(listing.paths ?? []);
    } catch (e) {
      setError(e);
      if ((e as { code?: string })?.code === "unauthorized")
        setConnected(false);
    }
  }, [path]);
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;
  useEffect(() => {
    localStorage.setItem("windowsMcpTransferLanguage", language);
    document.documentElement.lang = language;
    document.title = t("transfer") + " · Windows-MCP";
    const locale = language === "en" ? enUS : zhCN;
    uppyRef.current?.setOptions({ locale });
    uppyRef.current?.getPlugin("Dashboard")?.setOptions({ locale });
  }, [language]);
  useEffect(() => {
    void webRequest<{ name: string }>("/v1/info")
      .then((info) => {
        setHostName(info.name);
        setConnected(true);
      })
      .catch(() => {});
  }, []);
  useEffect(() => {
    if (!connected) return;
    void refresh();
  }, [connected, refresh]);
  useEffect(() => {
    if (!connected) return;
    let disposed = false;
    const update = async () => {
      try {
        const data = await webRequest<{ tasks: TransferTask[] }>("/v1/tasks");
        if (!disposed) setTasks(data.tasks);
      } catch (e) {
        if (!disposed) {
          setError(e);
          if ((e as { code?: string })?.code === "unauthorized")
            setConnected(false);
        }
      }
    };
    void update();
    const timer = setInterval(() => void update(), 1000);
    return () => {
      disposed = true;
      clearInterval(timer);
    };
  }, [connected]);
  useEffect(() => {
    if (!connected || !dashboard.current) return;
    const locale = languageRef.current === "en" ? enUS : zhCN;
    const uppy = new Uppy({
      locale,
      restrictions: { maxNumberOfFiles: 10000, maxFileSize: 1024 ** 4 },
    })
      .use(Dashboard, {
        inline: true,
        target: dashboard.current,
        height: 330,
        proudlyDisplayPoweredByUppy: false,
        locale,
        hideProgressDetails: false,
      })
      .use(XHRUpload, {
        method: "PUT",
        formData: false,
        limit: 2,
        timeout: 300000,
        endpoint: (file) => {
          if (Array.isArray(file)) throw new Error("Unexpected upload bundle");
          const relative = String(file.meta.relativePath ?? file.name);
          return fileUrl(
            [String(file.meta.destination ?? ""), relative]
              .filter(Boolean)
              .join("/"),
          );
        },
        headers: (file) => ({
          "X-Content-SHA256": String(file.meta.sha256 ?? ""),
        }),
        onAfterResponse: (xhr) => {
          if (xhr.status >= 400) {
            let failure: unknown = { code: "transfer_network" };
            try {
              failure = JSON.parse(xhr.responseText);
            } catch {
              /* Use the network error fallback. */
            }
            throw new Error(transferError(languageRef.current, failure));
          }
        },
      });
    uppyRef.current = uppy;
    uppy.on("file-added", (file) => {
      uppy.setFileMeta(file.id, { destination: pathRef.current });
    });
    uppy.addPreProcessor(async (ids) => {
      for (const id of ids) {
        const file = uppy.getFile(id);
        if (!file || !(file.data instanceof Blob)) continue;
        const hash = sha256.create();
        const size = file.data.size;
        try {
          for (let offset = 0; offset < size; offset += 4 * 1024 * 1024) {
            if (!uppy.getFile(id)) break;
            hash.update(
              new Uint8Array(
                await file.data
                  .slice(offset, offset + 4 * 1024 * 1024)
                  .arrayBuffer(),
              ),
            );
            uppy.emit("preprocess-progress", file, {
              mode: "determinate",
              message: transferText(languageRef.current, "hashing"),
              value: Math.min(1, (offset + 4 * 1024 * 1024) / size),
            });
          }
          if (uppy.getFile(id))
            uppy.setFileMeta(id, { sha256: bytesToHex(hash.digest()) });
        } finally {
          uppy.emit("preprocess-complete", file);
        }
      }
    });
    uppy.on("complete", () => void refreshRef.current());
    return () => {
      uppy.destroy();
      uppyRef.current = null;
    };
  }, [connected]);
  async function login(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await webRequest("/v1/session", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ token }),
      });
      setToken("");
      const info = await webRequest<{ name: string }>("/v1/info");
      setHostName(info.name);
      setConnected(true);
    } catch (e) {
      setError(e);
    } finally {
      setBusy(false);
    }
  }
  function selectFiles(list: FileList | null) {
    if (!list) return;
    for (const file of Array.from(list)) {
      try {
        uppyRef.current?.addFile({
          name: file.name,
          type: file.type,
          data: file,
          meta: {
            relativePath: file.webkitRelativePath || file.name,
            destination: pathRef.current,
          },
        });
      } catch (e) {
        setError(e);
      }
    }
  }
  async function showHash(name: string) {
    try {
      const response = await fetch(
        fileUrl([path, name].filter(Boolean).join("/"), "?hash"),
      );
      if (!response.ok) throw { code: "transfer_network" };
      setChecksum(await response.text());
    } catch (e) {
      setError(e);
    }
  }
  return (
    <div className="web-transfer">
      <header className="web-transfer-header">
        <div className="web-transfer-brand">
          <img className="brand-symbol" src={appIcon} alt="" />
          <div>
            <strong>Windows-MCP</strong>
            <small>
              {t("transfer")} · {hostName}
            </small>
          </div>
        </div>
        <div className="transfer-button-row">
          <Languages size={16} />
          <select
            aria-label={language === "en" ? "Interface language" : "界面语言"}
            value={language}
            onChange={(e) => setLanguage(e.target.value as Language)}
          >
            <option value="zh-CN">简体中文</option>
            <option value="en">English</option>
          </select>
          {connected && (
            <button
              className="button secondary"
              onClick={() =>
                void webRequest("/v1/session", { method: "DELETE" }).then(() =>
                  setConnected(false),
                )
              }
            >
              <LogOut size={15} />
              {t("disconnect")}
            </button>
          )}
        </div>
      </header>
      <main className="web-transfer-main">
        <div className="page-heading">
          <div>
            <div className="eyebrow">WINDOWS-MCP · FILE TRANSFER</div>
            <h1>{t("transfer")}</h1>
            <p>{t("noClient")}</p>
          </div>
          <span className="official-badge">
            <ShieldCheck size={16} />
            {t("noOverwrite")}
          </span>
        </div>
        {error !== null && (
          <div className="error-banner" role="alert">
            {transferError(language, error)}
          </div>
        )}
        {!connected ? (
          <form
            className="card transfer-login"
            onSubmit={(event) => void login(event)}
          >
            <div className="icon-box">
              <ShieldCheck size={23} />
            </div>
            <h2>{t("loginTitle")}</h2>
            <p>{t("loginHint")}</p>
            <label className="field-label" htmlFor="web-token">
              {t("token")}
            </label>
            <input
              id="web-token"
              type="password"
              autoComplete="off"
              placeholder={t("tokenRequired")}
              value={token}
              onChange={(e) => setToken(e.target.value)}
            />
            <button className="button primary full" disabled={busy || !token}>
              {t("connect")}
            </button>
          </form>
        ) : (
          <>
            <section className="card web-upload">
              <div className="card-heading">
                <span className="icon-box">
                  <ArrowUpDown size={19} />
                </span>
                <div>
                  <h2>{t("selectFiles")}</h2>
                  <p>{t("uploadHint")}</p>
                </div>
                <div className="transfer-button-row">
                  <button
                    className="button secondary"
                    onClick={() => fileInput.current?.click()}
                  >
                    {t("selectFiles")}
                  </button>
                  <button
                    className="button secondary"
                    onClick={() => folderInput.current?.click()}
                  >
                    {t("selectFolder")}
                  </button>
                </div>
              </div>
              <input
                hidden
                type="file"
                multiple
                ref={fileInput}
                aria-label={t("selectFiles")}
                onChange={(e) => {
                  selectFiles(e.target.files);
                  e.target.value = "";
                }}
              />
              <input
                hidden
                type="file"
                multiple
                {...{ webkitdirectory: "" }}
                ref={folderInput}
                aria-label={t("selectFolder")}
                onChange={(e) => {
                  selectFiles(e.target.files);
                  e.target.value = "";
                }}
              />
              <div ref={dashboard} />
              <p className="field-hint">{t("folderHint")}</p>
            </section>
            <section className="card web-files">
              <div className="card-heading">
                <span className="icon-box">
                  <Folder size={19} />
                </span>
                <div>
                  <h2>{t("files")}</h2>
                  <p>{t("downloadsHint")}</p>
                </div>
                <button
                  className="button secondary"
                  onClick={() => void refresh()}
                >
                  <RefreshCw size={15} />
                  {t("refresh")}
                </button>
              </div>
              <div className="transfer-file-toolbar">
                <code>/{path}</code>
                <div className="transfer-button-row">
                  {path && (
                    <button
                      className="button secondary"
                      onClick={() =>
                        setPath(
                          path
                            .split("/")
                            .slice(0, -1)
                            .join("" + "/"),
                        )
                      }
                    >
                      <ArrowUp size={15} />
                      {t("up")}
                    </button>
                  )}
                  <a
                    className="button secondary"
                    href={fileUrl(path, "?zip")}
                    download
                  >
                    <Download size={15} />
                    {t("folderZip")}
                  </a>
                </div>
              </div>
              <div className="transfer-file-list">
                {!files.length ? (
                  <div className="transfer-empty">
                    <Folder size={26} />
                    <p>{t("empty")}</p>
                  </div>
                ) : (
                  files.map((file) => {
                    const directory = file.path_type.endsWith("Dir"),
                      relative = [path, file.name].filter(Boolean).join("/");
                    return (
                      <div className="transfer-file" key={file.name}>
                        {directory ? <Folder size={20} /> : <File size={20} />}
                        <button
                          className="transfer-file-name"
                          onClick={() =>
                            directory
                              ? setPath(relative)
                              : void showHash(file.name)
                          }
                        >
                          {file.name}
                        </button>
                        <span>
                          {directory ? t("folder") : bytes(file.size)}
                        </span>
                        {!directory && (
                          <button
                            className="text-button"
                            onClick={() => void showHash(file.name)}
                          >
                            {t("checksum")}
                          </button>
                        )}
                        <a
                          className="icon-button"
                          aria-label={`${t("download")} ${file.name}`}
                          href={fileUrl(relative, directory ? "?zip" : "")}
                          download
                        >
                          <Download size={17} />
                        </a>
                      </div>
                    );
                  })
                )}
              </div>
              {checksum && (
                <div className="transfer-checksum">
                  <strong>{t("hashTitle")}</strong>
                  <code>{checksum}</code>
                  <button
                    className="text-button"
                    onClick={() => setChecksum("")}
                  >
                    ×
                  </button>
                </div>
              )}
            </section>
            <TransferTasks
              language={language}
              tasks={tasks}
              cancel={(id) =>
                void webRequest(`/v1/tasks/${encodeURIComponent(id)}`, {
                  method: "DELETE",
                }).catch(setError)
              }
            />
          </>
        )}
        <footer className="web-transfer-footer">{t("library")}</footer>
      </main>
    </div>
  );
}
