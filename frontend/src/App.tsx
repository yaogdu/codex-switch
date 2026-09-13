import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Activity,
  ArrowDownUp,
  CalendarDays,
  Check,
  ChevronLeft,
  ChevronRight,
  CircleStop,
  Download,
  FolderOpen,
  Globe2,
  Link2,
  Link2Off,
  LayoutList,
  LoaderCircle,
  Monitor,
  Moon,
  Pencil,
  Plus,
  Power,
  RefreshCw,
  Route,
  Search,
  ShieldCheck,
  Settings2,
  SlidersHorizontal,
  Sun,
  Trash2,
  X,
} from "lucide-react";
import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { useCallback } from "react";

type Page = "sessions" | "profiles";
type BindingMode = "global" | "fixed";
type RefreshMinutes = 0 | 1 | 2 | 3 | 5 | 10 | 15;
type ThemeMode = "system" | "light" | "dark";
type SessionFilter = "today" | "all";
type SessionProfileFilter = "all" | "global" | `profile:${string}`;

const refreshOptions: RefreshMinutes[] = [1, 2, 3, 5, 10, 15];

function initialRefreshMinutes(): RefreshMinutes {
  const value = Number(localStorage.getItem("codex-switch.refresh-minutes"));
  return [0, ...refreshOptions].includes(value as RefreshMinutes)
    ? (value as RefreshMinutes)
    : 5;
}

function initialThemeMode(): ThemeMode {
  const value = localStorage.getItem("codex-switch.theme");
  return value === "light" || value === "dark" || value === "system"
    ? value
    : "system";
}

type Profile = {
  id: string;
  base_url: string;
  is_default: boolean;
  auth_configured: boolean;
};

type Session = {
  id: string;
  title: string;
  project_dir: string;
  created_at: number;
  last_active_at: number;
  size_bytes: number;
  binding_mode: BindingMode | "unbound";
  profile_id: string | null;
};

type Dashboard = {
  proxy_running: boolean;
  listen: string | null;
  codex_proxy_enabled: boolean;
  default_profile: string;
  profiles: Profile[];
  sessions: Session[];
};

type ProfileForm = {
  id: string;
  base_url: string;
  api_key: string;
};

const sampleDashboard: Dashboard = {
  proxy_running: false,
  listen: null,
  codex_proxy_enabled: false,
  default_profile: "sakura",
  profiles: [
    { id: "aihezu", base_url: "未配置", is_default: false, auth_configured: false },
    { id: "her", base_url: "未配置", is_default: false, auth_configured: false },
    { id: "sakura", base_url: "未配置", is_default: true, auth_configured: false },
  ],
  sessions: [],
};

const isTauri = "__TAURI_INTERNALS__" in window;

async function command<T>(
  name: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  if (!isTauri) {
    throw new Error("请在 Tauri 应用中运行此操作");
  }
  return invoke<T>(name, args);
}

function App() {
  const [page, setPage] = useState<Page>("sessions");
  const [dashboard, setDashboard] = useState<Dashboard>(sampleDashboard);
  const [query, setQuery] = useState("");
  const [sessionFilter, setSessionFilter] = useState<SessionFilter>("today");
  const [profileFilter, setProfileFilter] =
    useState<SessionProfileFilter>("all");
  const [sort, setSort] = useState<
    "last_active_at" | "created_at" | "size_bytes"
  >(
    "last_active_at",
  );
  const [refreshMinutes, setRefreshMinutes] =
    useState<RefreshMinutes>(initialRefreshMinutes);
  const [themeMode, setThemeMode] = useState<ThemeMode>(initialThemeMode);
  const [sessionPage, setSessionPage] = useState(1);
  const pageSize = 20;
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [editingProfile, setEditingProfile] = useState<ProfileForm | null>(
    null,
  );
  const refreshRequest = useRef(0);

  const refresh = useCallback(async (silent = false) => {
    const requestId = ++refreshRequest.current;
    if (!silent) {
      setLoading(true);
    }
    try {
      const data = await command<Dashboard>("get_dashboard");
      if (requestId !== refreshRequest.current) return;
      setDashboard(data);
      if (!silent) {
        setNotice(null);
      }
    } catch (error) {
      if (requestId !== refreshRequest.current) return;
      if (isTauri && !silent) {
        setNotice(String(error));
      }
    } finally {
      if (!silent && requestId === refreshRequest.current) {
        setLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    document.documentElement.dataset.theme = themeMode;
    localStorage.setItem("codex-switch.theme", themeMode);
  }, [themeMode]);

  useEffect(() => {
    if (!isTauri) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<string | null>("codex-switch-state-changed", (event) => {
      void refresh(true);
      if (event.payload) {
        setNotice(event.payload);
      }
    }).then((dispose) => {
      if (active) {
        unlisten = dispose;
      } else {
        dispose();
      }
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [refresh]);

  useEffect(() => {
    const refreshWhenVisible = () => {
      if (
        refreshMinutes > 0 &&
        document.visibilityState === "visible"
      ) {
        void refresh(true);
      }
    };
    const interval =
      refreshMinutes > 0
        ? window.setInterval(refreshWhenVisible, refreshMinutes * 60_000)
        : undefined;
    window.addEventListener("focus", refreshWhenVisible);
    document.addEventListener("visibilitychange", refreshWhenVisible);
    return () => {
      if (interval) {
        window.clearInterval(interval);
      }
      window.removeEventListener("focus", refreshWhenVisible);
      document.removeEventListener("visibilitychange", refreshWhenVisible);
    };
  }, [refresh, refreshMinutes]);

  useEffect(() => {
    if (!notice) return;
    const timeout = window.setTimeout(() => setNotice(null), 2000);
    return () => window.clearTimeout(timeout);
  }, [notice]);

  const sortSessions = (
    a: Session,
    b: Session,
    key: "last_active_at" | "created_at" | "size_bytes",
  ) => {
    const primary = Number(b[key]) - Number(a[key]);
    if (primary !== 0) return primary;
    const recent = b.last_active_at - a.last_active_at;
    if (recent !== 0) return recent;
    const created = b.created_at - a.created_at;
    return created !== 0 ? created : a.id.localeCompare(b.id);
  };

  const sortedSessions = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    return [...dashboard.sessions]
      .filter((session) => {
        if (!matchesSessionProfile(session, profileFilter)) {
          return false;
        }
        if (sessionFilter === "today" && !isToday(session.last_active_at)) {
          return false;
        }
        if (!normalized) return true;
        return [session.id, session.title, session.project_dir]
          .join(" ")
          .toLowerCase()
          .includes(normalized);
      })
      .sort((a, b) => sortSessions(a, b, sort));
  }, [dashboard.sessions, profileFilter, query, sessionFilter, sort]);
  const todaySessionCount = useMemo(
    () =>
      dashboard.sessions.filter((session) => isToday(session.last_active_at))
        .length,
    [dashboard.sessions],
  );
  const totalSessionPages = Math.max(
    1,
    Math.ceil(sortedSessions.length / pageSize),
  );
  const visibleSessions = sortedSessions.slice(
    (sessionPage - 1) * pageSize,
    sessionPage * pageSize,
  );

  useEffect(() => {
    setSessionPage(1);
  }, [profileFilter, query, sessionFilter, sort]);

  useEffect(() => {
    if (
      profileFilter.startsWith("profile:") &&
      !dashboard.profiles.some(
        (profile) => `profile:${profile.id}` === profileFilter,
      )
    ) {
      setProfileFilter("all");
    }
  }, [dashboard.profiles, profileFilter]);

  useEffect(() => {
    if (sessionPage > totalSessionPages) {
      setSessionPage(totalSessionPages);
    }
  }, [sessionPage, totalSessionPages]);

  const toggleProxy = async () => {
    setBusy(true);
    try {
      await command(
        dashboard.proxy_running ? "stop_proxy" : "start_proxy",
      );
      await refresh();
      setNotice(dashboard.proxy_running ? "Proxy 已停止" : "Proxy 已启动");
    } catch (error) {
      setNotice(String(error));
    } finally {
      setBusy(false);
    }
  };

  const toggleCodexTakeover = async () => {
    setBusy(true);
    try {
      await command(
        dashboard.codex_proxy_enabled ? "restore_codex" : "takeover_codex",
      );
      await refresh();
      setNotice(
        dashboard.codex_proxy_enabled
          ? "已取消接管，Codex 已回到原配置"
          : "Codex 已接入本地 Proxy",
      );
    } catch (error) {
      setNotice(String(error));
    } finally {
      setBusy(false);
    }
  };

  const saveProfile = async (form: ProfileForm) => {
    setBusy(true);
    try {
      await command("save_profile", { profile: form });
      const id = form.id.trim();
      const baseUrl = form.base_url.trim().replace(/\/+$/, "");
      setDashboard((current) => {
        const previous = current.profiles.find((profile) => profile.id === id);
        const nextProfiles = [
          ...current.profiles.filter((profile) => profile.id !== id),
          {
            id,
            base_url: baseUrl,
            is_default: id === current.default_profile,
            auth_configured:
              Boolean(form.api_key.trim()) || Boolean(previous?.auth_configured),
          },
        ].sort((a, b) => a.id.localeCompare(b.id));
        return { ...current, profiles: nextProfiles };
      });
      setEditingProfile(null);
      void refresh(true);
      setNotice("配置档已保存");
    } catch (error) {
      setNotice(String(error));
    } finally {
      setBusy(false);
    }
  };

  const importProfiles = async () => {
    setBusy(true);
    try {
      const count = await command<number>("import_existing_profiles");
      await refresh();
      setNotice(`已导入 ${count} 个本机 Codex 配置档`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setBusy(false);
    }
  };

  const deleteProfile = async (profile: Profile) => {
    setBusy(true);
    try {
      await command("delete_profile", { profileId: profile.id });
      setDashboard((current) => {
        const profiles = current.profiles.filter((item) => item.id !== profile.id);
        const defaultProfile = profiles.some(
          (item) => item.id === current.default_profile,
        )
          ? current.default_profile
          : profiles[0]?.id ?? current.default_profile;
        return {
          ...current,
          default_profile: defaultProfile,
          profiles: profiles.map((item) => ({
            ...item,
            is_default: item.id === defaultProfile,
          })),
          sessions: current.sessions.map((session) =>
            session.profile_id === profile.id
              ? { ...session, binding_mode: "global", profile_id: null }
              : session,
          ),
        };
      });
      void refresh(true);
      setNotice("配置档已删除");
    } catch (error) {
      setNotice(String(error));
    } finally {
      setBusy(false);
    }
  };

  const setDefault = async (profileId: string) => {
    setBusy(true);
    try {
      await command("set_default_profile", { profileId });
      setDashboard((current) => ({
        ...current,
        default_profile: profileId,
        profiles: current.profiles.map((profile) => ({
          ...profile,
          is_default: profile.id === profileId,
        })),
      }));
      void refresh(true);
      setNotice(`默认配置档已切换为 ${profileId}`);
    } catch (error) {
      setNotice(String(error));
    } finally {
      setBusy(false);
    }
  };

  const setBinding = async (
    sessionId: string,
    mode: BindingMode,
    profileId?: string,
  ) => {
    try {
      await command("set_session_binding", {
        sessionId,
        mode,
        profileId: mode === "fixed" ? profileId : null,
      });
      await refresh();
    } catch (error) {
      setNotice(String(error));
    }
  };

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">
            <Route size={18} strokeWidth={2.2} />
          </div>
          <div>
            <strong>Codex Switch</strong>
            <span>LOCAL SESSION ROUTER</span>
          </div>
        </div>

        <nav className="nav-list" aria-label="主导航">
          <button
            className={page === "sessions" ? "nav-item active" : "nav-item"}
            onClick={() => setPage("sessions")}
          >
            <LayoutList size={17} />
            <span>Sessions</span>
            <b>{dashboard.sessions.length}</b>
          </button>
          <button
            className={page === "profiles" ? "nav-item active" : "nav-item"}
            onClick={() => setPage("profiles")}
          >
            <SlidersHorizontal size={17} />
            <span>配置档</span>
            <b>{dashboard.profiles.length}</b>
          </button>
        </nav>

        <div className="sidebar-footer">
          <div className="theme-switch" role="group" aria-label="主题">
            <button
              type="button"
              className={themeMode === "system" ? "active" : undefined}
              title="跟随系统"
              aria-label="跟随系统"
              aria-pressed={themeMode === "system"}
              onClick={() => setThemeMode("system")}
            >
              <Monitor size={14} />
            </button>
            <button
              type="button"
              className={themeMode === "light" ? "active" : undefined}
              title="浅色"
              aria-label="浅色"
              aria-pressed={themeMode === "light"}
              onClick={() => setThemeMode("light")}
            >
              <Sun size={14} />
            </button>
            <button
              type="button"
              className={themeMode === "dark" ? "active" : undefined}
              title="深色"
              aria-label="深色"
              aria-pressed={themeMode === "dark"}
              onClick={() => setThemeMode("dark")}
            >
              <Moon size={14} />
            </button>
          </div>
          <div className="proxy-mini">
            <span className={dashboard.proxy_running ? "status-dot on" : "status-dot"} />
            <div>
              <span>Local Proxy</span>
              <small>
                {dashboard.proxy_running
                  ? dashboard.listen ?? "运行中"
                  : "未启动"}
              </small>
            </div>
          </div>
        </div>
      </aside>

      <main className="main-panel">
        <header className="topbar">
          <div>
            <p className="eyebrow">LOCAL ROUTING CONTROL</p>
            <h1>{page === "sessions" ? "Sessions" : "配置档"}</h1>
          </div>
          <div className="topbar-actions">
            <div className="topbar-status">
              <span
                className={
                  dashboard.proxy_running ? "status-dot on" : "status-dot"
                }
              />
              <div>
                <strong>
                  {dashboard.proxy_running ? "Proxy 在线" : "Proxy 待启动"}
                </strong>
                <span>
                  {dashboard.codex_proxy_enabled
                    ? "Codex 已接入"
                    : "Codex 未接入"}
                </span>
              </div>
            </div>
            <button
              className="button button-quiet"
              onClick={() => void toggleCodexTakeover()}
              disabled={busy || (!dashboard.proxy_running && !dashboard.codex_proxy_enabled)}
              title={
                dashboard.proxy_running || dashboard.codex_proxy_enabled
                  ? undefined
                  : "请先启动 Proxy"
              }
            >
              {dashboard.codex_proxy_enabled ? (
                <Link2Off size={15} />
              ) : (
                <Link2 size={15} />
              )}
              {dashboard.codex_proxy_enabled ? "取消接管" : "接管 Codex"}
            </button>
            <button
              className={
                dashboard.proxy_running
                  ? "button button-danger"
                  : "button button-primary"
              }
              onClick={() => void toggleProxy()}
              disabled={busy}
            >
              {busy ? (
                <LoaderCircle className="spin" size={15} />
              ) : dashboard.proxy_running ? (
                <CircleStop size={15} />
              ) : (
                <Power size={15} />
              )}
              {dashboard.proxy_running ? "停止 Proxy" : "启动 Proxy"}
            </button>
          </div>
        </header>

        <section className="content">
          <div className="metric-strip">
            <Metric
              label="今日活跃"
              value={String(todaySessionCount)}
              detail={`全部 ${dashboard.sessions.length} 条记录`}
              icon={<Activity size={17} />}
            />
            <Metric
              label="配置档"
              value={String(dashboard.profiles.length)}
              detail={`默认使用 ${dashboard.default_profile}`}
              icon={<Settings2 size={17} />}
            />
            <Metric
              label="路由状态"
              value={dashboard.proxy_running ? "运行中" : "已停止"}
              detail={
                dashboard.codex_proxy_enabled
                  ? "Codex 已接入本地 Proxy"
                  : dashboard.listen ?? "启动后监听本地地址"
              }
              icon={<ShieldCheck size={17} />}
              accent={dashboard.proxy_running}
            />
          </div>

          {page === "sessions" ? (
            <section className="workspace">
              <div className="workspace-toolbar">
                <div className="toolbar-title">
                  <h2>Session 路由</h2>
                  <span>只管理元数据，不读取对话内容</span>
                </div>
                <div className="toolbar-controls">
                  <label className="search-box">
                    <Search size={15} />
                    <input
                      value={query}
                      onChange={(event) => {
                        setQuery(event.target.value);
                        setSessionPage(1);
                      }}
                      placeholder="搜索 ID、标题或目录"
                    />
                  </label>
                  <label className="select-box filter-select" title="按时间筛选">
                    <CalendarDays size={14} />
                    <select
                      value={sessionFilter}
                      onChange={(event) =>
                        setSessionFilter(event.target.value as SessionFilter)
                      }
                    >
                      <option value="today">今天活跃</option>
                      <option value="all">全部 Session</option>
                    </select>
                  </label>
                  <label
                    className="select-box profile-filter-select"
                    title="按配置档筛选"
                  >
                    <SlidersHorizontal size={14} />
                    <select
                      value={profileFilter}
                      onChange={(event) =>
                        setProfileFilter(
                          event.target.value as SessionProfileFilter,
                        )
                      }
                    >
                      <option value="all">所有配置档</option>
                      <option value="global">跟随全局</option>
                      {dashboard.profiles.map((profile) => (
                        <option
                          key={profile.id}
                          value={`profile:${profile.id}`}
                        >
                          固定：{profile.id}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label className="select-box">
                    <ArrowDownUp size={14} />
                    <select
                      value={sort}
                      onChange={(event) =>
                        (() => {
                          setSort(
                            event.target.value as
                              | "last_active_at"
                              | "created_at"
                              | "size_bytes",
                          );
                          setSessionPage(1);
                        })()
                      }
                    >
                      <option value="last_active_at">最后使用</option>
                      <option value="created_at">创建时间</option>
                      <option value="size_bytes">大小</option>
                    </select>
                  </label>
                  <button
                    className="icon-button"
                    title="刷新列表"
                    onClick={() => void refresh()}
                  >
                    <RefreshCw className={loading ? "spin" : ""} size={16} />
                  </button>
                  <label className="refresh-select" title="自动刷新间隔">
                    <RefreshCw size={13} />
                    <select
                      value={refreshMinutes}
                      onChange={(event) => {
                        const value = Number(
                          event.target.value,
                        ) as RefreshMinutes;
                        setRefreshMinutes(value);
                        localStorage.setItem(
                          "codex-switch.refresh-minutes",
                          String(value),
                        );
                      }}
                    >
                      {refreshOptions.map((minutes) => (
                        <option key={minutes} value={minutes}>
                          {minutes} 分钟
                        </option>
                      ))}
                      <option value={0}>不自动刷新</option>
                    </select>
                  </label>
                </div>
              </div>

              <div className="table-wrap">
                <table className="session-table">
                  <thead>
                    <tr>
                      <th>Session</th>
                      <th>Session ID</th>
                      <th>工作目录</th>
                      <th>大小</th>
                      <th>最后使用</th>
                      <th>路由策略</th>
                    </tr>
                  </thead>
                  <tbody>
                    {visibleSessions.map((session) => (
                      <SessionRow
                        key={session.id}
                        session={session}
                        profiles={dashboard.profiles}
                        onBinding={setBinding}
                      />
                    ))}
                  </tbody>
                </table>
                {sortedSessions.length === 0 && (
                  <EmptySessions
                    query={query}
                    sessionFilter={sessionFilter}
                    profileFilter={profileFilter}
                    onRefresh={() => void refresh()}
                  />
                )}
              </div>
              {sortedSessions.length > 0 && (
                <SessionPagination
                  page={sessionPage}
                  totalPages={totalSessionPages}
                  total={sortedSessions.length}
                  pageSize={pageSize}
                  onPageChange={setSessionPage}
                />
              )}
            </section>
          ) : (
            <section className="workspace">
              <div className="workspace-toolbar">
                <div className="toolbar-title">
                  <h2>配置档</h2>
                  <span>管理上游连接，不修改 Session 内容</span>
                </div>
                <div className="toolbar-controls">
                  <button
                    className="button button-quiet"
                    onClick={() => void importProfiles()}
                    disabled={busy}
                  >
                    <Download size={15} />
                    导入本机配置
                  </button>
                  <button
                    className="button button-primary"
                    onClick={() =>
                      setEditingProfile({ id: "", base_url: "", api_key: "" })
                    }
                  >
                    <Plus size={15} />
                    新建配置档
                  </button>
                </div>
              </div>
              <div className="profile-list">
                {dashboard.profiles.map((profile) => (
                  <ProfileRow
                    key={profile.id}
                    profile={profile}
                    onEdit={() =>
                      setEditingProfile({
                        id: profile.id,
                        base_url: profile.base_url,
                        api_key: "",
                      })
                    }
                    onDelete={() => void deleteProfile(profile)}
                    onDefault={() => void setDefault(profile.id)}
                  />
                ))}
              </div>
            </section>
          )}
        </section>
      </main>

      {notice && (
        <div className="toast" role="status">
          <Check size={15} />
          <span>{notice}</span>
          <button title="关闭提示" onClick={() => setNotice(null)}>
            <X size={14} />
          </button>
        </div>
      )}

      {editingProfile && (
        <ProfileDialog
          profile={editingProfile}
          busy={busy}
          onClose={() => setEditingProfile(null)}
          onSave={(form) => void saveProfile(form)}
        />
      )}
    </div>
  );
}

function Metric({
  label,
  value,
  detail,
  icon,
  accent = false,
}: {
  label: string;
  value: string;
  detail: string;
  icon: React.ReactNode;
  accent?: boolean;
}) {
  return (
    <div className={accent ? "metric accent" : "metric"}>
      <div className="metric-icon">{icon}</div>
      <div>
        <span>{label}</span>
        <strong>{value}</strong>
        <small>{detail}</small>
      </div>
    </div>
  );
}

function SessionRow({
  session,
  profiles,
  onBinding,
}: {
  session: Session;
  profiles: Profile[];
  onBinding: (
    sessionId: string,
    mode: BindingMode,
    profileId?: string,
  ) => Promise<void>;
}) {
  const [mode, setMode] = useState<BindingMode>(
    session.binding_mode === "fixed" ? "fixed" : "global",
  );
  const [profileId, setProfileId] = useState(
    session.profile_id ?? profiles.find((profile) => profile.is_default)?.id ?? "",
  );

  useEffect(() => {
    setMode(session.binding_mode === "fixed" ? "fixed" : "global");
    if (session.profile_id) {
      setProfileId(session.profile_id);
    }
  }, [session.binding_mode, session.profile_id]);

  const changeMode = async (nextMode: BindingMode) => {
    setMode(nextMode);
    await onBinding(session.id, nextMode, profileId);
  };

  const changeProfile = async (nextProfile: string) => {
    setProfileId(nextProfile);
    await onBinding(session.id, "fixed", nextProfile);
  };

  return (
    <tr>
      <td>
        <div className="session-primary">
          <strong title={session.title}>{session.title || "未命名 Session"}</strong>
        </div>
      </td>
      <td>
        <code className="session-id-cell" title={session.id}>
          {session.id}
        </code>
      </td>
      <td>
        <div className="path-cell" title={session.project_dir}>
          <FolderOpen size={14} />
          <span>{session.project_dir || "未知目录"}</span>
        </div>
      </td>
      <td className="muted-cell">{formatBytes(session.size_bytes)}</td>
      <td className="muted-cell">{formatDate(session.last_active_at)}</td>
      <td>
        <div className="route-control">
          <select
            value={mode}
            onChange={(event) =>
              void changeMode(event.target.value as BindingMode)
            }
          >
            <option value="global">跟随全局</option>
            <option value="fixed">固定配置档</option>
          </select>
          {mode === "fixed" && (
            <select
              value={profileId}
              onChange={(event) => void changeProfile(event.target.value)}
            >
              {profiles.map((profile) => (
                <option key={profile.id} value={profile.id}>
                  {profile.id}
                </option>
              ))}
            </select>
          )}
        </div>
      </td>
    </tr>
  );
}

function matchesSessionProfile(
  session: Session,
  profileFilter: SessionProfileFilter,
) {
  if (profileFilter === "all") return true;
  if (profileFilter === "global") return session.profile_id === null;
  return session.profile_id === profileFilter.slice("profile:".length);
}

function ProfileRow({
  profile,
  onEdit,
  onDelete,
  onDefault,
}: {
  profile: Profile;
  onEdit: () => void;
  onDelete: () => void;
  onDefault: () => void;
}) {
  return (
    <div className="profile-row">
      <div className="profile-symbol">{profile.id.slice(0, 2).toUpperCase()}</div>
      <div className="profile-main">
        <div className="profile-name">
          <strong>{profile.id}</strong>
          {profile.is_default && <span className="default-badge">默认</span>}
          <span className={profile.auth_configured ? "auth-badge ready" : "auth-badge"}>
            {profile.auth_configured ? "认证已配置" : "无认证"}
          </span>
        </div>
        <span title={profile.base_url}>{profile.base_url}</span>
      </div>
      <div className="profile-actions">
        {profile.is_default ? (
          <span className="current-default">
            <Globe2 size={13} />
            当前全局默认
          </span>
        ) : (
          <button className="text-button" onClick={onDefault}>
            <Globe2 size={13} />
            设为全局默认
          </button>
        )}
        <button className="icon-button" title="编辑配置档" onClick={onEdit}>
          <Pencil size={15} />
        </button>
        <button
          className="icon-button danger-icon"
          title="删除配置档"
          onClick={onDelete}
        >
          <Trash2 size={15} />
        </button>
      </div>
    </div>
  );
}

function ProfileDialog({
  profile,
  busy,
  onClose,
  onSave,
}: {
  profile: ProfileForm;
  busy: boolean;
  onClose: () => void;
  onSave: (profile: ProfileForm) => void;
}) {
  const [form, setForm] = useState(profile);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    onSave({
      id: form.id.trim(),
      base_url: form.base_url.trim(),
      api_key: form.api_key.trim(),
    });
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <form className="modal" onSubmit={submit} onMouseDown={(event) => event.stopPropagation()}>
        <div className="modal-header">
          <div>
            <span className="eyebrow">PROFILE</span>
            <h2>{profile.id ? "编辑配置档" : "新建配置档"}</h2>
          </div>
          <button type="button" className="icon-button" title="关闭" onClick={onClose}>
            <X size={17} />
          </button>
        </div>
        <label className="field">
          <span>名称</span>
          <input
            required
            disabled={Boolean(profile.id)}
            value={form.id}
            onChange={(event) => setForm({ ...form, id: event.target.value })}
            placeholder="例如 aihezu"
          />
        </label>
        <label className="field">
          <span>上游地址</span>
          <input
            required
            type="url"
            value={form.base_url}
            onChange={(event) =>
              setForm({ ...form, base_url: event.target.value })
            }
            placeholder="https://api.example.com/v1"
          />
        </label>
        <label className="field">
          <span>API Key（可选）</span>
          <input
            type="password"
            value={form.api_key}
            onChange={(event) =>
              setForm({ ...form, api_key: event.target.value })
            }
            placeholder={profile.id ? "留空则保留现有认证" : "sk-..."}
            autoComplete="new-password"
          />
        </label>
        <p className="field-note">
          凭据只写入本机应用配置文件（权限 600），不会显示在列表或日志中。已有 Codex OAuth 配置请使用“导入本机配置”。
        </p>
        <div className="modal-actions">
          <button type="button" className="button button-quiet" onClick={onClose}>
            取消
          </button>
          <button type="submit" className="button button-primary" disabled={busy}>
            {busy && <LoaderCircle className="spin" size={15} />}
            保存配置档
          </button>
        </div>
      </form>
    </div>
  );
}

function EmptySessions({
  query,
  sessionFilter,
  profileFilter,
  onRefresh,
}: {
  query: string;
  sessionFilter: SessionFilter;
  profileFilter: SessionProfileFilter;
  onRefresh: () => void;
}) {
  const isTodayFilter = sessionFilter === "today";
  const hasProfileFilter = profileFilter !== "all";
  return (
    <div className="empty-state">
      <div className="empty-icon">
        <LayoutList size={20} />
      </div>
      <strong>
        {query
          ? "没有匹配的 Session"
          : hasProfileFilter
            ? "没有匹配的 Session"
          : isTodayFilter
            ? "今天还没有活跃 Session"
            : "还没有扫描到 Session"}
      </strong>
      <span>
        {query
          ? "换一个关键词试试。"
          : hasProfileFilter
            ? "换一个配置档或清除筛选。"
          : isTodayFilter
            ? "切到全部 Session 查看历史记录。"
          : "确认本机存在 ~/.codex/sessions 后重新扫描。"}
      </span>
      {!query && (
        <button className="button button-quiet" onClick={onRefresh}>
          <RefreshCw size={15} />
          重新扫描
        </button>
      )}
    </div>
  );
}

function SessionPagination({
  page,
  totalPages,
  total,
  pageSize,
  onPageChange,
}: {
  page: number;
  totalPages: number;
  total: number;
  pageSize: number;
  onPageChange: (page: number) => void;
}) {
  const start = (page - 1) * pageSize + 1;
  const end = Math.min(page * pageSize, total);
  return (
    <div className="session-pagination">
      <span>
        显示 {start}-{end} / 共 {total} 条
      </span>
      <div className="pager-controls">
        <button
          className="icon-button"
          title="上一页"
          disabled={page === 1}
          onClick={() => onPageChange(page - 1)}
        >
          <ChevronLeft size={15} />
        </button>
        <span>
          第 {page} / {totalPages} 页
        </span>
        <button
          className="icon-button"
          title="下一页"
          disabled={page === totalPages}
          onClick={() => onPageChange(page + 1)}
        >
          <ChevronRight size={15} />
        </button>
      </div>
    </div>
  );
}

function formatBytes(bytes: number) {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1,
  );
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function formatDate(timestamp: number) {
  if (!timestamp) return "未知";
  return new Intl.DateTimeFormat("zh-CN", {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(timestamp));
}

function isToday(timestamp: number) {
  if (!timestamp) return false;
  const date = new Date(timestamp);
  const now = new Date();
  return (
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate()
  );
}

export default App;
