// System app types (built-in apps like chrome:// pages)
import type { SystemAppType as ActriumCoreSystemAppType } from './coreContract';

export type SystemAppType = ActriumCoreSystemAppType;

// History item passed from native
export type HistoryItemData = {
  id: string;
  title: string;
  appURL?: string;
  visitTime: number; // timestamp
};

// History group (closed tab group)
export type HistoryGroupData = {
  id: string;
  name: string;
  color: string;
  tabs: { title: string; appURL?: string }[];
  closedAt: number; // timestamp
};

// Bookmark item passed from native
export type BookmarkData = {
  id: string;
  title: string;
  appURL?: string;
  createdAt: number;
  lastUsed?: number;
};

export type TabConfig = {
  id: string;
  title: string;
  url?: string;
  /** Per-tab Service actor endpoint supplied by the Host after Attach. */
  serviceCallUrl?: string;
  /** Host-designated permission to use the legacy ACTR_CALL bridge. */
  legacyActorBridge?: boolean;
  htmlContent?: string;
  reloadKey?: number;
  contract?: {
    name: string;
    version: number;
  };
  permissions?: string[];
  /** Raw payload selected by `[workload.keel].capabilities` from the verified package. */
  capabilityManifestJSON?: string;
  // Legacy: Double Engine Structure
  guestScript?: string;
  leftPanelScript?: string;
  rightPanelScript?: string;
  // v2.0: Single Engine Structure
  unifiedScript?: string;
  backgroundColor: string;
  rightPanelDefaultVisible?: boolean;
  // System app type (built-in apps)
  systemAppType?: SystemAppType;
  // System app data
  historyItems?: HistoryItemData[];
  historyGroups?: HistoryGroupData[];
  bookmarks?: BookmarkData[];
  // Preset App URL, when the time is up LoadApp Autoload without manual user input
  appUrl?: string;
  appId?: string;
};

export type RuntimeProps = {
  tabs: TabConfig[];
  currentTabId?: string;
  // Native Size of the container passed (when ensuring window shrinks) RN I can feel it.
  containerWidth?: number;
  containerHeight?: number;
};

// TabRouter receives all tabs (both actr and system apps)
export type TabRouterProps = RuntimeProps & {
  // Single tab mode - each RootView renders one tab
  singleTabMode?: boolean;
  tab?: TabConfig; // Single tab when singleTabMode is true
  // Native Size of the container passed (when ensuring window shrinks) RN I can feel it.
  containerWidth?: number;
  containerHeight?: number;

  // FIXME These should be put in. tab Inside?
  // Preset App URL, when the time is up LoadApp Autoload without manual user input
  appUrl?: string;
  appId?: string;
  // Native-transparent payload for system app deep-links/actions
  systemAppPayload?: Record<string, unknown>;
};
