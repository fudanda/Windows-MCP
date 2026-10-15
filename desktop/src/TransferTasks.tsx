import type { Language } from "./i18n";
import type { TransferTask } from "./transfer-api";
import { bytes, transferError, transferText } from "./transfer-i18n";
import { CheckCircle2, Download, File, LoaderCircle, X } from "lucide-react";
export function TransferTasks({
  language,
  tasks,
  cancel,
}: {
  language: Language;
  tasks: TransferTask[];
  cancel: (id: string) => void;
}) {
  const t = (key: Parameters<typeof transferText>[1]) =>
    transferText(language, key);
  return (
    <section className="card transfer-tasks">
      <div className="card-heading">
        <span className="icon-box">
          <Download size={19} />
        </span>
        <div>
          <h2>{t("tasks")}</h2>
          <p>{t("completedHint")}</p>
        </div>
      </div>
      {!tasks.length ? (
        <div className="transfer-empty">
          <File size={25} />
          <p>{t("noTasks")}</p>
        </div>
      ) : (
        tasks.map((task) => {
          const active = !["completed", "failed", "cancelled"].includes(
            task.status,
          );
          const percent =
            task.status === "completed"
              ? 100
              : task.totalBytes
                ? Math.min(
                    100,
                    Math.round((task.transferredBytes / task.totalBytes) * 100),
                  )
                : 0;
          const seconds = Math.max(
            1,
            ((task.finishedAt ?? Date.now()) - task.startedAt) / 1000,
          );
          return (
            <article className={`transfer-task ${task.status}`} key={task.id}>
              <div className="transfer-task-heading">
                <div>
                  <strong>{task.name || t("preparing")}</strong>
                  <small>
                    {t(task.direction)} · {task.peer ?? "Browser"} ·{" "}
                    {task.id.slice(0, 8)}
                  </small>
                </div>
                <span className="transfer-task-status">
                  {task.status === "completed" ? (
                    <CheckCircle2 size={15} />
                  ) : active ? (
                    <LoaderCircle className="spin" size={15} />
                  ) : null}
                  {t(task.status)}
                </span>
                {active && (
                  <button
                    className="icon-button"
                    aria-label={`${t("cancel")} ${task.name}`}
                    onClick={() => cancel(task.id)}
                  >
                    <X size={16} />
                  </button>
                )}
              </div>
              <progress max={100} value={percent} aria-label={task.name} />
              <div className="transfer-task-facts">
                <span>
                  {bytes(task.transferredBytes)} /{" "}
                  {task.totalBytes ? bytes(task.totalBytes) : "—"}
                </span>
                <span>
                  {active
                    ? `${bytes(task.transferredBytes / seconds)}/s`
                    : `${percent}%`}
                </span>
                {task.fileCount !== undefined && (
                  <span>
                    {task.completedFiles ?? 0} / {task.fileCount}{" "}
                    {t("fileCount")}
                  </span>
                )}
              </div>
              {task.error && (
                <p className="transfer-task-error">
                  {transferError(language, task.error)}
                </p>
              )}
            </article>
          );
        })
      )}
    </section>
  );
}
