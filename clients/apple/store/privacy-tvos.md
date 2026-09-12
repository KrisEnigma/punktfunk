# tvOS — Privacy Policy text

App Store Connect asks for a **URL** on iOS and macOS but for **text** on tvOS: the Apple TV has
no browser, so the App Store shows the policy itself on the device. The field is required for the
tvOS platform, localisable per language, and ships with the next version like any other metadata.
Apple documents no character limit; `check-limits.py` holds it under 4000 like a description.

Paste the block for each language into **App Information → Privacy Policy for Apple TV**. It is
the app section of [`privacy-app-addendum.md`](privacy-app-addendum.md) made self-contained: it
adds the controller, the rights and complaint lines a standalone policy needs, and drops the
microphone paragraph because tvOS has no audio input. Keep the two in step.

## Deutsch (2765)

```
Datenschutzerklärung für Punktfunk auf Apple TV

Verantwortlicher
Enrico Bühler, unom, Schroffenstraße 44, 78628 Rottweil, Deutschland. E-Mail: buehler@unom.io

Die App erhebt keine personenbezogenen Daten
Es gibt keine Benutzerkonten, keine Registrierung und keine Anmeldung. Die App enthält keine Analyse-, Tracking-, Werbe- oder Absturzbericht-Bibliotheken von Drittanbietern. Es findet kein Tracking im Sinne des App Tracking Transparency Frameworks statt. Es werden keine Daten an uns oder an Dritte übermittelt.

Wohin die Daten fließen
Punktfunk verbindet Ihr Apple TV direkt mit einem Host-Rechner, den Sie selbst betreiben – in der Regel in Ihrem eigenen Netzwerk. Video, Ton sowie Controller-, Fernbedienungs-, Maus- und Tastatureingaben werden ausschließlich zwischen Ihrem Apple TV und diesem Host übertragen, verschlüsselt und ohne Umweg über einen Server von uns. Wir betreiben keine Vermittlungs-, Relay- oder Cloud-Dienste und haben zu keinem Zeitpunkt Zugriff auf die Inhalte einer Sitzung.

Was auf dem Gerät bleibt
Die App speichert lokal auf Ihrem Apple TV: die von Ihnen hinzugefügten oder im Netzwerk gefundenen Hosts, Ihre Einstellungen und Profile sowie einen kryptografischen Schlüssel im Schlüsselbund, mit dem sich Ihr Apple TV gegenüber einem gekoppelten Host ausweist. Diese Daten verlassen Ihr Gerät nicht und werden gelöscht, wenn Sie die App entfernen.

Lokales Netzwerk
Die App nutzt das lokale Netzwerk, um Hosts zu finden, sie per Wake-on-LAN zu wecken und sich mit ihnen zu verbinden. Andere Berechtigungen fragt sie nicht ab.

App Store
Apple verarbeitet im Rahmen der Auslieferung über den App Store eigene Daten, etwa Kauf-, Installations- und Absturzstatistiken. Darauf haben wir keinen Einfluss; es gilt die Datenschutzerklärung von Apple. Die aggregierten Statistiken, die Apple uns anzeigt, lassen keinen Rückschluss auf einzelne Personen zu.

Der Host
Der Punktfunk-Host ist quelloffene Software, die Sie selbst auf Ihrem eigenen Rechner betreiben. Welche Daten dabei anfallen, etwa lokale Protokolldateien, bleibt vollständig unter Ihrer Kontrolle; wir erhalten davon nichts. Der Quellcode ist unter git.unom.io/unom/punktfunk einsehbar.

Ihre Rechte
Da wir keine Daten über Sie speichern, gibt es bei uns nichts einzusehen, zu berichtigen oder zu löschen. Die lokal gespeicherten Daten löschen Sie, indem Sie die App entfernen. Fragen richten Sie an buehler@unom.io. Sie haben das Recht, sich bei einer Datenschutz-Aufsichtsbehörde zu beschweren (Art. 77 DSGVO); für uns zuständig ist der Landesbeauftragte für den Datenschutz und die Informationsfreiheit Baden-Württemberg.

Die vollständige Datenschutzerklärung, auch für unsere Website, finden Sie unter punktfunk.unom.io/de/legal/privacy.

Stand: 12. September 2026
```

## English (2470)

```
Privacy Policy for Punktfunk on Apple TV

Controller
Enrico Bühler, unom, Schroffenstraße 44, 78628 Rottweil, Germany. Email: buehler@unom.io

The app collects no personal data
There are no user accounts, no registration, and no sign-in. The app contains no third-party analytics, tracking, advertising, or crash-reporting libraries. No tracking within the meaning of Apple's App Tracking Transparency framework takes place. No data is sent to us or to any third party.

Where your data goes
Punktfunk connects your Apple TV directly to a host machine that you run yourself, normally on your own network. Video, audio, and controller, remote, mouse, and keyboard input travel only between your Apple TV and that host, encrypted, without passing through any server of ours. We operate no brokering, relay, or cloud service, and we have no access to the contents of a session at any point.

What stays on your device
The app stores locally on your Apple TV: the hosts you have added or discovered on your network, your settings and profiles, and a cryptographic key in the keychain that your Apple TV uses to identify itself to a paired host. This data does not leave your device and is removed when you delete the app.

Local network
The app uses the local network to find hosts, wake them with Wake-on-LAN, and connect to them. It requests no other permissions.

App Store
Apple processes its own data as part of distributing the app through the App Store, such as purchase, installation, and crash statistics. We have no influence over this, and Apple's privacy policy applies. The aggregated statistics Apple shows us do not allow any individual to be identified.

The host
The Punktfunk host is open source software that you run on your own machine. Any data it produces, such as local log files, remains entirely under your control, and none of it reaches us. The source is available at git.unom.io/unom/punktfunk.

Your rights
Since we store no data about you, there is nothing held by us to access, correct, or delete. You delete the locally stored data by removing the app. Questions go to buehler@unom.io. You have the right to lodge a complaint with a data protection supervisory authority (Art. 77 GDPR); the authority responsible for us is the State Commissioner for Data Protection and Freedom of Information of Baden-Württemberg.

The full privacy policy, including for our website, is at punktfunk.unom.io/en/legal/privacy.

Last updated: 12 September 2026
```
