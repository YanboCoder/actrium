const BASE = "/admin/api";
const TOKEN_KEY = "loran_admin_token";

async function request<T>(
  path: string,
  options: RequestInit = {},
): Promise<T> {
  const token = localStorage.getItem(TOKEN_KEY);
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    ...(options.headers as Record<string, string> | undefined),
  };
  if (token) {
    headers["Authorization"] = `Bearer ${token}`;
  }

  const res = await fetch(`${BASE}${path}`, { ...options, headers });

  if (res.status === 401) {
    localStorage.removeItem(TOKEN_KEY);
    window.location.href = "/admin/login";
    throw new Error("Unauthorized");
  }

  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || body.error_message || res.statusText);
  }

  return res.json();
}

// Types
export interface LoginResponse {
  token: string;
  expires_in: number;
}

export interface PublicNodeNameResponse {
  name: string;
}

export interface AdminCapabilities {
  realm_writes_enabled: boolean;
  superv_managed: boolean;
}

export interface MetricDetail {
  value: number;
  score: number;
}

export interface ServiceStatus {
  name: string;
  type: number;
  is_healthy: boolean;
  active_connections: number;
  total_requests: number;
  failed_requests: number;
  average_latency_ms: number;
  url: string | null;
  port: number | null;
  domain: string | null;
}

export interface NodeInfo {
  success: boolean;
  node_id: string;
  name: string;
  version: string;
  location_tag: string;
  uptime_secs: number;
  power_reserve: number;
  metrics: Record<string, MetricDetail> | null;
  services: ServiceStatus[];
}

export type RealmStatus = "pending" | "active" | "retired";

export interface RealmInfo {
  realm: string;
  /** false = suspended by the operator */
  enabled: boolean;
  created_at: number;
  updated_at: number | null;
  expires_at: number;
  status: RealmStatus;
}

export interface RealmsResponse {
  success: boolean;
  realms: RealmInfo[];
  total_count: number;
}

export interface RealmMutationResponse {
  success: boolean;
  error_message: string | null;
  realm?: RealmInfo | null;
  /** The owner invite of a created Realm, returned once. */
  owner_invite?: string | null;
  /** RFC 3339. */
  owner_invite_expires_at?: string | null;
}

export interface OwnerInviteResponse {
  success: boolean;
  realm: string;
  owner_invite: string;
  /** RFC 3339. */
  expires_at: string;
}

/** A member's client: its Realm client key, without the key itself. */
export interface RealmClientInfo {
  key_id: string;
  label: string | null;
  /** RFC 3339. */
  registered_at: string;
  last_used_at: string | null;
  revoked_at: string | null;
}

export interface RealmMemberInfo {
  member_id: string;
  role: "owner" | "editor" | "viewer";
  status: "active" | "removed";
  /** RFC 3339. */
  created_at: string;
  removed_at: string | null;
  clients: RealmClientInfo[];
}

export interface RealmMembersResponse {
  success: boolean;
  members: RealmMemberInfo[];
  /** Open invites, without their secrets. */
  invites: { invite_id: string; expires_at: string }[];
}

export interface ResolvedField {
  key: string;
  value_type: string;
  description: string;
  dynamic: boolean;
  reloadable: boolean;
  default_value: string;
  config_file_value: string | null;
  override_value: string | null;
  effective_value: string;
  source: "default" | "config_file" | "override";
  choices?: string[];
}

export interface ConfigFieldDef {
  key: string;
  toml_path: string;
  value_type: string;
  default_value: string;
  dynamic: boolean;
  reloadable: boolean;
  description: string;
  service: string;
}

export interface ConfigOverrideEntry {
  key_path: string;
  value: string;
  updated_at: string;
  updated_by: string;
}

export interface ServiceDetail {
  enabled: boolean;
  status: ServiceStatus | null;
  config: Record<string, unknown> | null;
  config_fields?: ResolvedField[];
}

export interface PlatformDetail {
  config: Record<string, unknown>;
  config_fields: ResolvedField[];
}

export interface KeyEntry {
  key_id: number;
  pk_size: number;
  created_at: number;
  retired: boolean;
}

export interface MetricSample {
  ts: number;
  active_conns: number;
  requests: number;
  failed_requests: number;
  latency_p95_ms: number;
}

export interface TimeseriesResponse {
  service_type: number;
  tier: number;
  interval_secs: number;
  samples: MetricSample[];
}

export const api = {
  login: (password: string) =>
    request<LoginResponse>("/auth/login", {
      method: "POST",
      body: JSON.stringify({ password }),
    }),

  getPublicNodeName: async (): Promise<PublicNodeNameResponse> => {
    const fallback = await fetch("/admin/health", {
      headers: { Accept: "application/json" },
    });
    if (fallback.ok) {
      const body = await fallback.json().catch(() => null);
      const name = typeof body?.node === "string"
        ? body.node.trim()
        : "";
      if (name.length > 0) {
        return { name };
      }
    }

    throw new Error("Failed to load node name");
  },

  getNodeInfo: () => request<NodeInfo>("/node"),

  getCapabilities: () => request<AdminCapabilities>("/capabilities"),

  getServices: () =>
    request<{ services: ServiceStatus[] }>("/node/services"),

  listRealms: () => request<RealmsResponse>("/realms"),

  /** A Realm under the Provider domain needs the package of its first Realm Profile. */
  createRealm: (data: {
    realm: string;
    enabled?: boolean;
    display_name?: string;
    package?: { href: string; sha256: string; type: string };
  }) =>
    request<RealmMutationResponse>("/realms", {
      method: "POST",
      body: JSON.stringify(data),
    }),

  getRealm: (realm: string) =>
    request<{ success: boolean; realm: RealmInfo | null }>(
      `/realms/${encodeURIComponent(realm)}`,
    ),

  /** `enabled: false` suspends the Realm, `true` resumes it. */
  updateRealm: (realm: string, data: { enabled?: boolean; expires_at?: number }) =>
    request<RealmMutationResponse>(`/realms/${encodeURIComponent(realm)}`, {
      method: "PUT",
      body: JSON.stringify(data),
    }),

  retireRealm: (realm: string) =>
    request<RealmMutationResponse>(`/realms/${encodeURIComponent(realm)}/retire`, {
      method: "POST",
    }),

  /** A new owner invite (owner recovery); unused earlier ones stop working. */
  issueOwnerInvite: (realm: string) =>
    request<OwnerInviteResponse>(`/realms/${encodeURIComponent(realm)}/owner-invite`, {
      method: "POST",
    }),

  /** Members, clients and open invites of a Realm, read-only. */
  listRealmMembers: (realm: string) =>
    request<RealmMembersResponse>(`/realms/${encodeURIComponent(realm)}/members`),

  getConfig: (configType: number, key: string) =>
    request<{
      success: boolean;
      config_value: string | null;
    }>(`/config/${configType}/${key}`),

  updateConfig: (configType: number, key: string, value: string) =>
    request<{ success: boolean; old_value: string | null }>(
      `/config/${configType}/${key}`,
      { method: "PUT", body: JSON.stringify({ config_value: value }) },
    ),

  getConfigFile: () =>
    request<{ content: string; path: string }>("/config-file"),

  saveConfigFile: (content: string) =>
    request<{ saved: boolean; error?: string }>("/config-file", {
      method: "PUT",
      body: JSON.stringify({ content }),
    }),

  reloadNode: () =>
    request<{ accepted: boolean }>("/node/reload", { method: "POST" }),

  restartNode: () =>
    request<{ accepted: boolean; error_message?: string }>("/node/restart", {
      method: "POST",
    }),

  shutdown: (data: {
    graceful?: boolean;
    timeout_secs?: number;
    reason?: string;
  }) =>
    request<{ accepted: boolean }>("/node/shutdown", {
      method: "POST",
      body: JSON.stringify(data),
    }),

  getPlatformDetail: () =>
    request<PlatformDetail>("/platform"),

  getServiceDetail: (name: string) =>
    request<ServiceDetail>(`/services/${name}`),

  getSignerKeys: () =>
    request<{ keys: KeyEntry[]; total_count: number }>("/services/signer/keys"),

  getRegistry: () =>
    request<ConfigFieldDef[]>("/registry"),

  getOverrides: () =>
    request<ConfigOverrideEntry[]>("/config/overrides"),

  setOverride: (key: string, value: string) =>
    request<{ success: boolean }>(`/config/overrides/${encodeURIComponent(key)}`, {
      method: "PUT",
      body: JSON.stringify({ value }),
    }),

  deleteOverride: (key: string) =>
    request<{ success: boolean; deleted: boolean }>(`/config/overrides/${encodeURIComponent(key)}`, {
      method: "DELETE",
    }),

  probePort: (port: number) =>
    request<{ reachable: boolean; latency_ms?: number; error?: string }>(`/network/probe/${port}`),

  getMetricsTimeseries: (serviceType: number, tier: number) =>
    request<TimeseriesResponse>(`/metrics/timeseries?service_type=${serviceType}&tier=${tier}`),
};

// Publisher Registry API types.
export interface PublisherRecord {
  id: number;
  name: string;
  public_key: string;
  key_id: string;
  contact?: string;
  status: 'active' | 'suspended' | 'revoked';
  created_at: number;
  updated_at?: number;
  verified_at?: number;
  suspended_at?: number;
  revoked_at?: number;
  key_expires_at?: number;
}

export interface PublisherKeyHistory {
  id: number;
  publisher_id: number;
  key_id: string;
  public_key: string;
  status: 'retired' | 'revoked';
  created_at: number;
  retired_at: number;
}

// Publisher Registry API functions, mounted under /admin/api/publishers.
export const publisherApi = {
  list: (status?: string) => {
    const params = status ? `?status=${status}` : '';
    return request<PublisherRecord[]>(`/publishers/admin/list${params}`);
  },

  suspend: (id: number) =>
    request<void>(`/publishers/admin/${id}/suspend`, { method: 'POST' }),

  reinstate: (id: number) =>
    request<void>(`/publishers/admin/${id}/reinstate`, { method: 'POST' }),

  delete: (id: number) =>
    request<void>(`/publishers/admin/${id}`, { method: 'DELETE' }),

  listKeys: (publisherId: number) =>
    request<PublisherKeyHistory[]>(`/publishers/admin/${publisherId}/keys`),

  revokeHistoricalKey: (historyId: number) =>
    request<void>(`/publishers/admin/keys/${historyId}/revoke`, { method: 'POST' }),

  revokeActiveKey: (publisherId: number) =>
    request<void>(`/publishers/admin/${publisherId}/keys/active/revoke`, { method: 'POST' }),
};

// Identity re-confirmation: pinned vs served signing keys of a Realm domain
// or a publisher (authority-discovery section 4.4 step 6).
export type IdentityKind = 'realm-domains' | 'publishers';

export interface IdentityKeys {
  sequence: number;
  digest: string;
  keys: { id: string; status: 'active' | 'retired' | 'removed'; fingerprint: string }[];
}

export interface IdentityView {
  pinned: IdentityKeys;
  fetched: IdentityKeys;
  outcome: 'accepted' | 'identity_change' | 'rejected';
  reason?: string;
}

export const identityApi = {
  get: (kind: IdentityKind, name: string) =>
    request<IdentityView>(`/identity/${kind}/${encodeURIComponent(name)}`),

  confirm: (kind: IdentityKind, name: string, digest: string) =>
    request<{ pinned: IdentityKeys }>(`/identity/${kind}/${encodeURIComponent(name)}`, {
      method: 'POST',
      body: JSON.stringify({ digest }),
    }),
};
