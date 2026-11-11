/**
 * iOS / Android entry point.
 *
 * These platforms will be rebuilt later; keep a minimal empty surface so the
 * native shells can still start without the legacy TabRouter runtime.
 */
import 'react-native/Libraries/Core/InitializeCore';

import React from 'react';
import { AppRegistry, View } from 'react-native';

function EmptyApp() {
  return React.createElement(View, {
    style: { flex: 1, backgroundColor: '#000000' },
  });
}

AppRegistry.registerComponent('actrium', () => EmptyApp);
