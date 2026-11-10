import { useState, type FormEvent, useEffect } from "react";
import { api, type RealmMutationResponse } from "../../lib/api";
import { X, Copy, Download, CheckCircle, AlertTriangle } from "lucide-react";

interface RealmFormProps {
  onSubmit: (data: Parameters<typeof api.createRealm>[0]) => Promise<RealmMutationResponse>;
  onClose: () => void;
  onRefresh?: () => void;
}

/**
 * Creates a Realm. A Realm name is fixed at creation and never reused. A
 * Realm under the Provider domain gets a Loran-signed Realm Profile, which
 * names its package.
 */
export function RealmForm({ onSubmit, onClose, onRefresh }: RealmFormProps) {
  const [name, setName] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [packageHref, setPackageHref] = useState("");
  const [packageSha256, setPackageSha256] = useState("");
  const [packageType, setPackageType] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  // Wizard state
  const [step, setStep] = useState<"input" | "success">("input");
  const [ownerInvite, setOwnerInvite] = useState<string | null>(null);
  const [inviteExpiresAt, setInviteExpiresAt] = useState<string | null>(null);
  const [createdRealm, setCreatedRealm] = useState<string | null>(null);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
      } else if (e.key === "Enter" && step === "success") {
        e.preventDefault();
        handleActivate();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose, step, createdRealm]); // Add step dependency

  // The Realm is created suspended so the owner invite can be saved before
  // it serves Actors; the second step resumes it.
  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);

    try {
      const resp = await onSubmit({
        realm: name,
        enabled: false,
        ...(displayName ? { display_name: displayName } : {}),
        ...(packageHref
          ? { package: { href: packageHref, sha256: packageSha256, type: packageType } }
          : {}),
      });
      if (resp.owner_invite) {
        setOwnerInvite(resp.owner_invite);
        setInviteExpiresAt(resp.owner_invite_expires_at ?? null);
        setCreatedRealm(resp.realm?.realm ?? name);
        setStep("success");
      } else {
        onClose();
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "Operation failed");
    } finally {
      setLoading(false);
    }
  }

  async function handleActivate() {
    if (!createdRealm) return;
    setLoading(true);
    try {
      await api.updateRealm(createdRealm, { enabled: true });
      if (onRefresh) onRefresh();
      onClose();
    } catch {
      setError("Failed to resume realm");
    } finally {
      setLoading(false);
    }
  }

  function handleDownload() {
    if (!ownerInvite || !createdRealm) return;
    const blob = new Blob([ownerInvite], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `realm-${createdRealm}-owner-invite.txt`;
    a.click();
    URL.revokeObjectURL(url);
  }

  function handleCopy() {
    if (ownerInvite) {
      navigator.clipboard.writeText(ownerInvite);
    }
  }

  if (step === "success" && ownerInvite && createdRealm) {
    return (
      <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40">
        <div className="w-full max-w-md rounded-xl bg-white p-6 shadow-xl">
          <div className="flex items-center justify-between mb-6">
            <h2 className="text-lg font-semibold text-gray-900 flex items-center gap-2">
              <CheckCircle className="h-5 w-5 text-green-500" />
              Realm Created
            </h2>
          </div>

          <div className="space-y-6">
            <div className="rounded-lg bg-amber-50 border border-amber-200 p-4">
              <div className="flex items-start gap-3">
                <AlertTriangle className="h-5 w-5 text-amber-600 shrink-0 mt-0.5" />
                <div className="space-y-1">
                  <p className="text-sm font-medium text-amber-900">Save the owner invite now</p>
                  <p className="text-xs text-amber-700">
                    It is shown only once. Hand it to the Realm&apos;s owner, who redeems it with a
                    new client and then invites the other members.
                  </p>
                </div>
              </div>
            </div>

            <div className="space-y-3">
              <div>
                <label className="text-xs font-medium text-gray-500 uppercase">Realm Name</label>
                <p className="text-sm font-mono text-gray-900">{createdRealm}</p>
              </div>
              <div>
                <label className="text-xs font-medium text-gray-500 uppercase">Owner Invite</label>
                <div className="mt-1 flex items-center gap-2">
                  <code className="flex-1 rounded bg-gray-100 px-3 py-2 text-sm font-mono text-gray-800 border border-gray-200 break-all">
                    {ownerInvite}
                  </code>
                </div>
                {inviteExpiresAt && (
                  <p className="mt-1 text-xs text-gray-500">
                    Expires {new Date(inviteExpiresAt).toLocaleString()}.
                  </p>
                )}
                <div className="flex gap-2 mt-2">
                  <button
                    onClick={handleCopy}
                    className="flex items-center gap-1.5 rounded border border-gray-300 px-3 py-1.5 text-xs font-medium text-gray-700 hover:bg-gray-50 transition-colors"
                  >
                    <Copy className="h-3.5 w-3.5" /> Copy
                  </button>
                  <button
                    onClick={handleDownload}
                    className="flex items-center gap-1.5 rounded border border-gray-300 px-3 py-1.5 text-xs font-medium text-gray-700 hover:bg-gray-50 transition-colors"
                  >
                    <Download className="h-3.5 w-3.5" /> Download
                  </button>
                </div>
              </div>
            </div>

            <div className="pt-4 border-t border-gray-100 flex flex-col gap-3">
              <button
                onClick={handleActivate}
                disabled={loading}
                className="w-full rounded-lg bg-blue-600 px-4 py-2.5 text-sm font-medium text-white hover:bg-blue-700 transition-colors disabled:opacity-50"
              >
                {loading ? "Resuming..." : "I have saved the invite, resume Realm"}
              </button>
              <button
                onClick={onClose}
                className="text-xs text-gray-500 hover:text-gray-700 hover:underline text-center"
              >
                Keep suspended for now
              </button>
            </div>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40">
      <div className="w-full max-w-md rounded-xl bg-white p-6 shadow-xl">
        <div className="flex items-center justify-between mb-4">
          <h2 className="text-lg font-semibold text-gray-900">
            Create Realm
          </h2>
          <button
            onClick={onClose}
            className="rounded-lg p-1 text-gray-400 hover:bg-gray-100 hover:text-gray-600 transition-colors"
          >
            <X className="h-5 w-5" />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Realm Name
            </label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value.trim())}
              placeholder="realm-id@example.com"
              title="<realm_id>@<domain>"
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500 disabled:bg-gray-50 disabled:text-gray-500 font-mono"
              required
              autoFocus
            />
            <p className="mt-1 text-xs text-gray-500">
              The name cannot be changed later, and a retired name is never reused.
            </p>
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Display Name <span className="font-normal text-gray-400">(optional)</span>
            </label>
            <input
              type="text"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
            />
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Package</label>
            <input
              type="url"
              value={packageHref}
              onChange={(e) => setPackageHref(e.target.value.trim())}
              placeholder="https://cdn.example.com/app-1.0.0.actr"
              className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500 font-mono"
            />
            <input
              type="text"
              value={packageSha256}
              onChange={(e) => setPackageSha256(e.target.value.trim().toLowerCase())}
              placeholder="SHA-256 (64 hex)"
              pattern="[0-9a-f]{64}"
              required={packageHref !== ""}
              className="mt-2 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500 font-mono"
            />
            <input
              type="text"
              value={packageType}
              onChange={(e) => setPackageType(e.target.value.trim())}
              placeholder="publisher.example/app@1.0.0"
              title="Actor type of the package manifest"
              required={packageHref !== ""}
              className="mt-2 w-full rounded-lg border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500 font-mono"
            />
            <p className="mt-1 text-xs text-gray-500">
              Required for Realms under this Actr Facility Provider&apos;s domain: it goes into the
              Realm Profile this instance signs. The type is the package manifest&apos;s
              <code> publisher/program@version</code>.
            </p>
          </div>

          {error && (
            <p className="text-sm text-red-600">{error}</p>
          )}

          <div className="flex justify-end gap-3 pt-2">
            <button
              type="button"
              onClick={onClose}
              className="rounded-lg border border-gray-300 px-4 py-2 text-sm font-medium text-gray-700 hover:bg-gray-50 transition-colors"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={loading}
              className="rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-50 transition-colors"
            >
              {loading ? "Saving..." : "Next"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
