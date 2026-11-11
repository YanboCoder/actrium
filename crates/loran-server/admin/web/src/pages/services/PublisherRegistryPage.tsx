import { useState, useEffect, useCallback } from 'react';
import { Building2, CheckCircle, Clock, XCircle, AlertTriangle, Key, ShieldOff, Timer } from 'lucide-react';
import { publisherApi, type PublisherRecord, type PublisherKeyHistory } from '../../lib/api';
import { IdentityReview } from '../../components/identity/IdentityReview';

// ── Status badge ──────────────────────────────────────────────────

function StatusBadge({ status }: { status: PublisherRecord['status'] }) {
  const config = {
    active: { color: 'bg-green-100 text-green-800', icon: CheckCircle, label: 'Active' },
    suspended: { color: 'bg-orange-100 text-orange-800', icon: AlertTriangle, label: 'Suspended' },
    revoked: { color: 'bg-red-100 text-red-800', icon: XCircle, label: 'Revoked' },
  }[status];
  const Icon = config.icon;
  return (
    <span className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium ${config.color}`}>
      <Icon size={12} />
      {config.label}
    </span>
  );
}

// ── Key Expiry Badge ──────────────────────────────────────────────

function KeyExpiryBadge({ expiresAt }: { expiresAt?: number }) {
  if (!expiresAt) {
    return <span className="text-gray-300 text-xs">—</span>;
  }

  const now = Date.now() / 1000;
  const remaining = expiresAt - now;
  const days = Math.ceil(remaining / 86400);
  const expiryDate = new Date(expiresAt * 1000);
  const dateStr = expiryDate.toLocaleDateString();

  let colorClass: string;
  let label: string;
  let Icon = Timer;

  if (remaining <= 0) {
    // Expired
    colorClass = 'bg-red-600 text-white';
    label = 'Expired';
    Icon = XCircle;
  } else if (days <= 7) {
    // ≤ 7 days — critical
    colorClass = 'bg-red-100 text-red-800 ring-1 ring-red-300';
    label = `${days}d left`;
    Icon = AlertTriangle;
  } else if (days <= 15) {
    // ≤ 15 days — warning
    colorClass = 'bg-orange-100 text-orange-800 ring-1 ring-orange-300';
    label = `${days}d left`;
    Icon = AlertTriangle;
  } else if (days <= 30) {
    // ≤ 30 days — attention
    colorClass = 'bg-yellow-100 text-yellow-800 ring-1 ring-yellow-200';
    label = `${days}d left`;
    Icon = Clock;
  } else {
    // Healthy
    colorClass = 'bg-green-50 text-green-700';
    label = dateStr;
    Icon = CheckCircle;
  }

  return (
    <span
      className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium whitespace-nowrap ${colorClass}`}
      title={`Expires: ${expiryDate.toLocaleString()}${remaining > 0 ? ` (${days} days remaining)` : ' (EXPIRED)'}`}
    >
      <Icon size={11} />
      {label}
    </span>
  );
}

// ── Key History Panel ────────────────────────────────────────────────

function KeyHistoryPanel({ publisher, onRevoked }: { publisher: PublisherRecord; onRevoked: () => void }) {
  const [keys, setKeys] = useState<PublisherKeyHistory[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [revoking, setRevoking] = useState<number | null>(null);

  useEffect(() => {
    setLoading(true);
    publisherApi.listKeys(publisher.id)
      .then(setKeys)
      .catch(e => setError(String(e)))
      .finally(() => setLoading(false));
  }, [publisher.id]);

  const handleRevoke = async (k: PublisherKeyHistory) => {
    if (!confirm(`Revoke key ${k.key_id}?\n\nThis is an emergency action. All packages signed with this key will fail verification immediately.`)) return;
    setRevoking(k.id);
    try {
      await publisherApi.revokeHistoricalKey(k.id);
      setKeys(prev => prev ? prev.map(x => x.id === k.id ? { ...x, status: 'revoked' } : x) : prev);
      onRevoked();
    } catch (e) {
      setError(String(e));
    } finally {
      setRevoking(null);
    }
  };

  const ts = (t: number) => new Date(t * 1000).toLocaleDateString();

  return (
    <div className="bg-gray-50 border-t border-gray-100 px-6 py-4">
      <div className="flex items-center gap-2 mb-3">
        <Key size={13} className="text-gray-400" />
        <span className="text-xs font-semibold text-gray-600 uppercase">Key History — {publisher.name}</span>
        <span className="text-xs text-gray-400 ml-1">(Retired keys still verify old packages until manually revoked)</span>
      </div>
      {loading && <div className="text-xs text-gray-400">Loading...</div>}
      {error && <div className="text-xs text-red-600">{error}</div>}
      {!loading && keys !== null && keys.length === 0 && (
        <div className="text-xs text-gray-400">No historical keys.</div>
      )}
      {!loading && keys && keys.length > 0 && (
        <table className="w-full text-xs">
          <thead>
            <tr className="text-gray-400 uppercase">
              <th className="text-left pb-1 pr-4 font-medium">Key ID</th>
              <th className="text-left pb-1 pr-4 font-medium">Status</th>
              <th className="text-left pb-1 pr-4 font-medium">Active from</th>
              <th className="text-left pb-1 pr-4 font-medium">Retired at</th>
              <th className="text-left pb-1 font-medium">Action</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-100">
            {keys.map(k => (
              <tr key={k.id} className="hover:bg-white">
                <td className="py-1.5 pr-4 font-mono text-gray-800">{k.key_id}</td>
                <td className="py-1.5 pr-4">
                  {k.status === 'retired' ? (
                    <span className="inline-flex items-center gap-1 px-1.5 py-0.5 bg-gray-100 rounded text-gray-600">
                      <Clock size={10} /> retired
                    </span>
                  ) : (
                    <span className="inline-flex items-center gap-1 px-1.5 py-0.5 bg-red-100 rounded text-red-700">
                      <XCircle size={10} /> revoked
                    </span>
                  )}
                </td>
                <td className="py-1.5 pr-4 text-gray-500">{ts(k.created_at)}</td>
                <td className="py-1.5 pr-4 text-gray-500">{ts(k.retired_at)}</td>
                <td className="py-1.5">
                  {k.status === 'retired' && (
                    <button
                      onClick={() => void handleRevoke(k)}
                      disabled={revoking === k.id}
                      title="Emergency revoke: packages signed with this key will fail verification"
                      className="inline-flex items-center gap-1 px-2 py-0.5 bg-red-500 text-white rounded text-[11px] hover:bg-red-600 disabled:opacity-50"
                    >
                      <ShieldOff size={10} /> Revoke
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

// ── Registration ──────────────────────────────────────────────────

function HowItWorks() {
  return (
    <div className="bg-white rounded-xl border border-gray-200 p-4 text-sm text-gray-600 space-y-2">
      <div className="font-semibold text-gray-800">How publishers register</div>
      <p>
        A publisher signs a Publisher Profile and serves it as the <code className="bg-gray-100 px-1 rounded">publisher</code> member
        of <code className="bg-gray-100 px-1 rounded">{'https://<publisher>/.well-known/actr'}</code>. It then sends a
        <code className="bg-gray-100 px-1 mx-1 rounded">register_publisher</code> request, signed by an active Publisher Profile
        signing key, to this Actr Facility Provider&apos;s <code className="bg-gray-100 px-1 rounded">realm-management</code> endpoint.
      </p>
      <p>
        The first accepted profile pins its signing keys; its active package signing keys verify the publisher&apos;s packages.
        A key rotation is a later profile with a higher sequence. Replaced keys still verify old packages until revoked here.
      </p>
    </div>
  );
}

// ── Main component ────────────────────────────────────────────────

export function PublisherRegistryPage() {
  const [publishers, setPublishers] = useState<PublisherRecord[]>([]);
  const [selectedPublisher, setSelectedPublisher] = useState<PublisherRecord | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [actionLoading, setActionLoading] = useState<number | null>(null);

  const loadData = useCallback(async () => {
    try {
      setPublishers(await publisherApi.list());
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void loadData(); }, [loadData]);

  const handleSuspend = async (publisher: PublisherRecord) => {
    if (!confirm(`Suspend "${publisher.name}"?`)) return;
    setActionLoading(publisher.id);
    try { await publisherApi.suspend(publisher.id); await loadData(); }
    catch (e) { setError(String(e)); }
    finally { setActionLoading(null); }
  };

  const handleReinstate = async (publisher: PublisherRecord) => {
    setActionLoading(publisher.id);
    try { await publisherApi.reinstate(publisher.id); await loadData(); }
    catch (e) { setError(String(e)); }
    finally { setActionLoading(null); }
  };

  const handleDelete = async (publisher: PublisherRecord) => {
    if (!confirm(`Delete "${publisher.name}" and its signing-key history? This cannot be undone.`)) return;
    setActionLoading(publisher.id);
    try { await publisherApi.delete(publisher.id); await loadData(); }
    catch (e) { setError(String(e)); }
    finally { setActionLoading(null); }
  };

  const handleRevokeActiveKey = async (publisher: PublisherRecord) => {
    if (!confirm(`Revoke the current package signing key of "${publisher.name}" (${publisher.key_id})?\n\nThis is an emergency action for a compromised key. Packages signed with it fail verification at once. Register a Publisher Profile with a new key to restore package verification.`)) return;
    setActionLoading(publisher.id);
    try { await publisherApi.revokeActiveKey(publisher.id); await loadData(); }
    catch (e) { setError(String(e)); }
    finally { setActionLoading(null); }
  };

  const stats = {
    total: publishers.length,
    active: publishers.filter(m => m.status === 'active').length,
    suspended: publishers.filter(m => m.status === 'suspended').length,
  };

  if (loading) return <div className="p-8 text-gray-500">Loading...</div>;

  return (
    <div className="p-6 space-y-6">
      <div className="flex items-start justify-between">
        <div>
          <h1 className="text-2xl font-bold text-gray-900 flex items-center gap-2">
            <Building2 size={24} /> Publisher Registry
          </h1>
          <p className="text-gray-500 text-sm mt-1">Review registered publishers and their signing keys.</p>
        </div>
      </div>

      <HowItWorks />

      {error && (
        <div className="bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700">{error}</div>
      )}

      {/* Stats */}
      <div className="grid grid-cols-3 gap-4">
        {[
          { label: 'Total', value: stats.total, color: 'text-gray-700' },
          { label: 'Active', value: stats.active, color: 'text-green-700' },
          { label: 'Suspended', value: stats.suspended, color: 'text-orange-700' },
        ].map(s => (
          <div key={s.label} className="bg-white rounded-xl border border-gray-200 p-4">
            <div className={`text-2xl font-bold ${s.color}`}>{s.value}</div>
            <div className="text-gray-500 text-sm">{s.label}</div>
          </div>
        ))}
      </div>

      {/* Publisher Registry Table */}
      <div className="bg-white rounded-xl border border-gray-200 overflow-hidden">
        <div className="px-4 py-3 border-b border-gray-100 flex items-center justify-between">
          <h2 className="font-semibold text-gray-800">Publishers</h2>
          {selectedPublisher && (
            <button onClick={() => setSelectedPublisher(null)} className="text-xs text-gray-500 hover:text-gray-800">
              Close key history
            </button>
          )}
        </div>
        <table className="w-full text-sm">
          <thead className="bg-gray-50 text-gray-500 text-xs uppercase">
            <tr>
              {['Name', 'Status', 'Key ID', 'Key Expires', 'Actions'].map(h => (
                <th key={h} className="px-4 py-2 text-left font-medium">{h}</th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-100">
            {publishers.length === 0 && (
              <tr><td colSpan={5} className="px-4 py-8 text-center text-gray-400">No publishers registered</td></tr>
            )}
            {publishers.map(publisher => {
              const isSelected = selectedPublisher?.id === publisher.id;
              return (
                <>
                  <tr
                    key={publisher.id}
                    className={`hover:bg-gray-50 cursor-pointer ${isSelected ? 'bg-blue-50' : ''}`}
                    onClick={() => setSelectedPublisher(isSelected ? null : publisher)}
                  >
                    <td className="px-4 py-3 font-mono font-medium text-gray-900">{publisher.name}</td>
                    <td className="px-4 py-3"><StatusBadge status={publisher.status} /></td>
                    <td className="px-4 py-3 text-gray-500 font-mono text-[11px] truncate max-w-[120px]" title={publisher.key_id}>{publisher.key_id || '—'}</td>
                    <td className="px-4 py-3">
                      <KeyExpiryBadge expiresAt={publisher.key_expires_at} />
                    </td>
                    <td className="px-4 py-3" onClick={e => e.stopPropagation()}>
                      <div className="flex gap-1">
                        {publisher.status === 'active' && (
                          <button
                            onClick={() => void handleSuspend(publisher)}
                            disabled={actionLoading === publisher.id}
                            className="px-2 py-1 text-xs bg-orange-500 text-white rounded hover:bg-orange-600 disabled:opacity-50"
                          >Suspend</button>
                        )}
                        {publisher.status === 'active' && publisher.key_id && (
                          <button
                            onClick={() => void handleRevokeActiveKey(publisher)}
                            disabled={actionLoading === publisher.id}
                            title="Emergency revoke of the current package signing key"
                            className="inline-flex items-center gap-1 px-2 py-1 text-xs bg-red-700 text-white rounded hover:bg-red-800 disabled:opacity-50"
                          ><ShieldOff size={11} /> Revoke key</button>
                        )}
                        {publisher.status === 'suspended' && (
                          <button
                            onClick={() => void handleReinstate(publisher)}
                            disabled={actionLoading === publisher.id}
                            className="px-2 py-1 text-xs bg-green-600 text-white rounded hover:bg-green-700 disabled:opacity-50"
                          >Reinstate</button>
                        )}
                        <button
                          onClick={() => void handleDelete(publisher)}
                          disabled={actionLoading === publisher.id}
                          className="px-2 py-1 text-xs bg-red-500 text-white rounded hover:bg-red-600 disabled:opacity-50"
                        >Delete</button>
                      </div>
                    </td>
                  </tr>
                  {isSelected && (
                    <tr key={`${publisher.id}-history`}>
                      <td colSpan={5} className="p-0">
                        <KeyHistoryPanel publisher={publisher} onRevoked={() => void loadData()} />
                      </td>
                    </tr>
                  )}
                </>
              );
            })}
          </tbody>
        </table>
      </div>

      <IdentityReview kind="publishers" />
    </div>
  );
}
