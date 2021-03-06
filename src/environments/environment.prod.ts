// This file can be replaced during build by using the `fileReplacements` array.
// `ng build --prod` replaces `environment.ts` with `environment.prod.ts`.
// The list of file replacements can be found in `angular.json`.
const environment = {
  appName: 'Leapp',
  sessionDuration: 1200,
  lockFileDestination: '',
  python3Version: '3.4.0',
  production: true,
  credentialsDestination: '.aws/credentials',
  azureAccessTokens: '.azure/accessTokens.json',
  azureProfile: '.azure/azureProfile.json'
};

environment.lockFileDestination = `.Leapp/Leapp-lock.json`;
export {environment};

