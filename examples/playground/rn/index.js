/**
 * @format
 */

// First: @undra/react-native installs what Hermes lacks before anything loads @undra/runtime.
import '@undra/react-native';
import { AppRegistry } from 'react-native';
import App from './src/App';
import { name as appName } from './app.json';

AppRegistry.registerComponent(appName, () => App);
