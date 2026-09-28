# Changelog

## [0.24.0](https://github.com/Arskah/radiodiodj/compare/v0.23.0...v0.24.0) (2026-09-26)


### Features

* **library:** measure each track's musical key ([#489](https://github.com/Arskah/radiodiodj/issues/489)) ([45deada](https://github.com/Arskah/radiodiodj/commit/45deadaf52ac7beaa0223f58d7edf5033ee77b59))
* **library:** measure each track's tempo ([#479](https://github.com/Arskah/radiodiodj/issues/479)) ([47d50f7](https://github.com/Arskah/radiodiodj/commit/47d50f7e047ebc6c05a029c10c911983b6500958))


### Documentation

* draw the boundary around audio measurement ([#482](https://github.com/Arskah/radiodiodj/issues/482)) ([35268e9](https://github.com/Arskah/radiodiodj/commit/35268e9aa080535ce68540b239371197f7a39721))


### Code Refactoring

* **audio_measure:** stop the level envelope naming cue points ([#486](https://github.com/Arskah/radiodiodj/issues/486)) ([814b93e](https://github.com/Arskah/radiodiodj/commit/814b93e19475a2909d791f7a955d2c167471ce66))
* **audio:** move what a decode measures into audio_measure ([#483](https://github.com/Arskah/radiodiodj/issues/483)) ([9e17876](https://github.com/Arskah/radiodiodj/commit/9e17876f87f4596932bcc467ad351a4558bbff27))
* **library:** move the fingerprint in with the measurements ([#484](https://github.com/Arskah/radiodiodj/issues/484)) ([d855b7e](https://github.com/Arskah/radiodiodj/commit/d855b7eb3367a382c22b85e6a52fd4ab6a1759dc))


### Tests

* **audio_measure:** make the boundary fail rather than rot ([#485](https://github.com/Arskah/radiodiodj/issues/485)) ([69b9cd8](https://github.com/Arskah/radiodiodj/commit/69b9cd8a383ac15d83d863cc169a0578a8a8bfb5))

## [0.23.0](https://github.com/Arskah/radiodiodj/compare/v0.22.0...v0.23.0) (2026-09-25)


### Features

* **library:** backfill tag columns a row predates ([#464](https://github.com/Arskah/radiodiodj/issues/464)) ([a7ab78e](https://github.com/Arskah/radiodiodj/commit/a7ab78e967ba575f7df710606febb7cb57e967a2))
* **library:** edit the new tag fields and write them back ([#466](https://github.com/Arskah/radiodiodj/issues/466)) ([fb94ad3](https://github.com/Arskah/radiodiodj/commit/fb94ad3a3682589ecd8a1ec2fc0634f42f418e58))
* **library:** read the rest of the tag metadata ([#463](https://github.com/Arskah/radiodiodj/issues/463)) ([3ced312](https://github.com/Arskah/radiodiodj/commit/3ced3125a2fce440be963ed0d14d14266324051b))
* **library:** recalculate automatic cue points on demand ([#454](https://github.com/Arskah/radiodiodj/issues/454)) ([b1e4c35](https://github.com/Arskah/radiodiodj/commit/b1e4c35100da4833da137d73660149f3b5d97b3a))
* **library:** show the track number and sort by album order ([#465](https://github.com/Arskah/radiodiodj/issues/465)) ([d30b83d](https://github.com/Arskah/radiodiodj/commit/d30b83d7e741178020041af001d685223fb5f827))
* **library:** store the level envelope with every automatic cue result ([#453](https://github.com/Arskah/radiodiodj/issues/453)) ([c4d50fb](https://github.com/Arskah/radiodiodj/commit/c4d50fb7974853c8dc10b1e2bfd2bd1daab853ae))


### Bug Fixes

* **deps:** update dependency material-symbols to v0.47.5 ([#462](https://github.com/Arskah/radiodiodj/issues/462)) ([2a36a9a](https://github.com/Arskah/radiodiodj/commit/2a36a9a639d63661c10dc23fd151cc6feca347b9))
* **library:** let the fingerprint decide what a rescan invalidates ([#470](https://github.com/Arskah/radiodiodj/issues/470)) ([02fec20](https://github.com/Arskah/radiodiodj/commit/02fec20a2794f3fa6d1896d4f1f61fcb0b2b479a)), closes [#460](https://github.com/Arskah/radiodiodj/issues/460)
* **library:** let the operator stop the analysis pass ([#468](https://github.com/Arskah/radiodiodj/issues/468)) ([9b341dd](https://github.com/Arskah/radiodiodj/commit/9b341ddc2af66bb0412db20935cbb8e5376d1951)), closes [#455](https://github.com/Arskah/radiodiodj/issues/455)
* **library:** read cover-art-heavy MP3s; split the settings overlay ([#472](https://github.com/Arskah/radiodiodj/issues/472)) ([b9f39fb](https://github.com/Arskah/radiodiodj/commit/b9f39fb99e79de35f870ecad07a71ba5e69d0ce9))
* **settings:** let the Library tab read its own directories ([#477](https://github.com/Arskah/radiodiodj/issues/477)) ([defc263](https://github.com/Arskah/radiodiodj/commit/defc263a1483c10040c437580fe6548ffe002ffd))


### Miscellaneous Chores

* clear out dead code, stray docs and untokenized radii ([#473](https://github.com/Arskah/radiodiodj/issues/473)) ([64484d6](https://github.com/Arskah/radiodiodj/commit/64484d6a9442821c7835a956ff99fb3b3f4c77e7))
* **deps:** update dependency jsdom to v30.1.1 ([#461](https://github.com/Arskah/radiodiodj/issues/461)) ([d316896](https://github.com/Arskah/radiodiodj/commit/d316896cb36567342ab2dfed20aac2e360ed3121))
* **deps:** update pnpm to v12.6.0 ([#469](https://github.com/Arskah/radiodiodj/issues/469)) ([9495328](https://github.com/Arskah/radiodiodj/commit/94953284cc9058112e16cb28bf6b95f42222e4ef))
* lint the comment conventions instead of remembering them ([#478](https://github.com/Arskah/radiodiodj/issues/478)) ([ffb00de](https://github.com/Arskah/radiodiodj/commit/ffb00de0c0c51a600a5c359d5329cdc18d6e253a))


### Documentation

* catch the docs up with what v1 actually ships ([#471](https://github.com/Arskah/radiodiodj/issues/471)) ([f012100](https://github.com/Arskah/radiodiodj/commit/f012100462c8898f0bb7c2edc3aa175b1b6e0d30))


### Code Refactoring

* **audio:** reduce a decode to a per-window level envelope ([#452](https://github.com/Arskah/radiodiodj/issues/452)) ([0a0d342](https://github.com/Arskah/radiodiodj/commit/0a0d342914be9dce8f942133c70d1baec904570c))
* **library:** give db.rs one way to build an id list ([#474](https://github.com/Arskah/radiodiodj/issues/474)) ([e791433](https://github.com/Arskah/radiodiodj/commit/e791433c1fff7a02d6a37ebc41e88d5a8502340f))
* **state:** one race guard behind the four deck fetches ([#475](https://github.com/Arskah/radiodiodj/issues/475)) ([25ef654](https://github.com/Arskah/radiodiodj/commit/25ef65454326cb8ca59d4a1f8c790b84ea5b5604))
* **ui:** share the scrim, card and header the three dialogs had copied ([#476](https://github.com/Arskah/radiodiodj/issues/476)) ([7556469](https://github.com/Arskah/radiodiodj/commit/75564698cb2989afd0999bc4abfb3fba0acd4c5b))

## [0.22.0](https://github.com/Arskah/radiodiodj/compare/v0.21.2...v0.22.0) (2026-09-24)


### ⚠ BREAKING CHANGES

* **deps:** every stored track fingerprint is invalidated. On the first run of this build the background analysis pass re-fingerprints every present track, one extra megabyte read per track. Tracks that are missing at that moment keep their old fingerprint and come back as new tracks if their file reappears at another path, losing the cue points, play count and edits attached to the old row. Present tracks keep everything; the database is not reset.

### Features

* **audio:** level every track to the ReplayGain reference ([#429](https://github.com/Arskah/radiodiodj/issues/429)) ([990f7bc](https://github.com/Arskah/radiodiodj/commit/990f7bcced7dc50d365e52719b8c1b88f4838aad)), closes [#80](https://github.com/Arskah/radiodiodj/issues/80)
* **library:** derive cue points from the analysis decode ([#431](https://github.com/Arskah/radiodiodj/issues/431)) ([c2ae235](https://github.com/Arskah/radiodiodj/commit/c2ae2355b33173c4cc311e0da0f2823cc4e8ce70)), closes [#372](https://github.com/Arskah/radiodiodj/issues/372)
* **library:** switch automatic Next starts off on their own ([#450](https://github.com/Arskah/radiodiodj/issues/450)) ([3ce2768](https://github.com/Arskah/radiodiodj/commit/3ce2768a45eacb96595a45174c7fb4bed7dda523))
* **playlist:** keep the auto-playlist off recent titles and artists ([#430](https://github.com/Arskah/radiodiodj/issues/430)) ([bbb083d](https://github.com/Arskah/radiodiodj/commit/bbb083ddf5edc345c56d2659491166f8ebfc78c9)), closes [#282](https://github.com/Arskah/radiodiodj/issues/282)


### Bug Fixes

* **deps:** update dependency @tauri-apps/plugin-log to v2.9.2 ([#447](https://github.com/Arskah/radiodiodj/issues/447)) ([23cbedc](https://github.com/Arskah/radiodiodj/commit/23cbedcb9ee9eb952295f89ee38bc3f0d9561b89))
* **deps:** update dependency material-symbols to v0.47.4 ([#437](https://github.com/Arskah/radiodiodj/issues/437)) ([e5d3ffe](https://github.com/Arskah/radiodiodj/commit/e5d3ffe8be6fe359a385e43f85e9c3e4202315a1))
* **deps:** update rust crate lofty to v0.25.4 ([#442](https://github.com/Arskah/radiodiodj/issues/442)) ([bac2e4e](https://github.com/Arskah/radiodiodj/commit/bac2e4e8b2d8a1106ad1e7e948a11b866a529a6d))
* **deps:** update rust crate symphonia to v0.6.1 ([#377](https://github.com/Arskah/radiodiodj/issues/377)) ([6c4a3e2](https://github.com/Arskah/radiodiodj/commit/6c4a3e2fd9713393476d9d58aba2d8dece3a2a60))
* **deps:** update rust crate tauri-plugin-log to v2.9.2 ([#448](https://github.com/Arskah/radiodiodj/issues/448)) ([6d19cc3](https://github.com/Arskah/radiodiodj/commit/6d19cc30a98d3e45c4d05c538481a3d2dd71fc99))
* **deps:** update tauri monorepo ([#445](https://github.com/Arskah/radiodiodj/issues/445)) ([2125ecd](https://github.com/Arskah/radiodiodj/commit/2125ecdf38dd10cd905b2fdd7bb90693ee40bb35))
* **library:** escape quotes in search queries ([#424](https://github.com/Arskah/radiodiodj/issues/424)) ([c5b9e53](https://github.com/Arskah/radiodiodj/commit/c5b9e538d1c4f3534a857031ad4443f2ebe25c2a))
* **playlist:** put the interrupted track back on when the share returns ([#432](https://github.com/Arskah/radiodiodj/issues/432)) ([874370f](https://github.com/Arskah/radiodiodj/commit/874370fe3033799ed81789d61f66c56b516fc49a))


### Miscellaneous Chores

* **deps:** lock file maintenance ([#435](https://github.com/Arskah/radiodiodj/issues/435)) ([d44da45](https://github.com/Arskah/radiodiodj/commit/d44da455a8df2e1f294eb78d548fe3b8c48aa2c8))
* **deps:** update commitlint monorepo to v21.2.3 ([#444](https://github.com/Arskah/radiodiodj/issues/444)) ([f8e8d0f](https://github.com/Arskah/radiodiodj/commit/f8e8d0fe5ab2ab7776f4133a79e463088782d941))
* **deps:** update dependency @types/node to v25.9.8 ([#439](https://github.com/Arskah/radiodiodj/issues/439)) ([0ab2e9e](https://github.com/Arskah/radiodiodj/commit/0ab2e9ec214d7f65cbc0a3d67a22deaee4faf3c3))
* **deps:** update dependency eslint to v10.11.0 ([#441](https://github.com/Arskah/radiodiodj/issues/441)) ([aa7063f](https://github.com/Arskah/radiodiodj/commit/aa7063fd19edf0ffc481568cc0d60558c6f84c8e))
* **deps:** update dependency jsdom to v30.1.0 ([#433](https://github.com/Arskah/radiodiodj/issues/433)) ([471b44b](https://github.com/Arskah/radiodiodj/commit/471b44be3451ffd06b64fc9ce685fa13cff11a4f))
* **deps:** update dependency prettier to v3.9.8 ([#436](https://github.com/Arskah/radiodiodj/issues/436)) ([e955fad](https://github.com/Arskah/radiodiodj/commit/e955fad81d7c55e1f1945375e08d76125390e4d7))
* **deps:** update dependency svelte to v5.57.1 ([#440](https://github.com/Arskah/radiodiodj/issues/440)) ([466a5c2](https://github.com/Arskah/radiodiodj/commit/466a5c29f4e65e83125e34c5d819fc205331e95e))
* **deps:** update dependency tsx to v4.23.15 ([#446](https://github.com/Arskah/radiodiodj/issues/446)) ([e47bde3](https://github.com/Arskah/radiodiodj/commit/e47bde37c9bf557482f0598a0f7af8b858ca43ef))
* **deps:** update dependency typescript-eslint to v8.70.1 ([#451](https://github.com/Arskah/radiodiodj/issues/451)) ([9060555](https://github.com/Arskah/radiodiodj/commit/9060555aa219f1f15ca5b9859bdd837332f3f471))
* **deps:** update pnpm to v12.5.1 ([#438](https://github.com/Arskah/radiodiodj/issues/438)) ([d06071b](https://github.com/Arskah/radiodiodj/commit/d06071bce8f9286ba7d49b6b60a8cff3e8141815))
* **deps:** update webdriverio monorepo to v9.32.0 ([#449](https://github.com/Arskah/radiodiodj/issues/449)) ([31d858c](https://github.com/Arskah/radiodiodj/commit/31d858c129e3cae9b6224d8928be0417231ae206))
* **ui:** remove the decorative Live On Air badge ([#428](https://github.com/Arskah/radiodiodj/issues/428)) ([23994ef](https://github.com/Arskah/radiodiodj/commit/23994ef78bbd27e58e6c97d4af678b8965a47fff)), closes [#287](https://github.com/Arskah/radiodiodj/issues/287)


### Documentation

* design for fuzzy library search ([#425](https://github.com/Arskah/radiodiodj/issues/425)) ([cf6fa6f](https://github.com/Arskah/radiodiodj/commit/cf6fa6f3bda931e4d73362807091e44ebc5a9576))
* split README, AGENTS.md and docs/ by audience ([#434](https://github.com/Arskah/radiodiodj/issues/434)) ([cab8997](https://github.com/Arskah/radiodiodj/commit/cab89974db549f9b28307458f508ed55ce74cd4e))


### Continuous Integration

* run CI on pull requests only ([#457](https://github.com/Arskah/radiodiodj/issues/457)) ([17a6542](https://github.com/Arskah/radiodiodj/commit/17a65423675313c75d53d2027be18502e8c7f2d7))

## [0.21.2](https://github.com/Arskah/radiodiodj/compare/v0.21.1...v0.21.2) (2026-09-19)


### Bug Fixes

* **deps:** update dependency material-symbols to v0.47.3 ([#412](https://github.com/Arskah/radiodiodj/issues/412)) ([a95290c](https://github.com/Arskah/radiodiodj/commit/a95290cd0b03f4b9ab41604ea105f799ac7c6363))


### Miscellaneous Chores

* **deps:** update dependency @types/node to v25.9.7 ([#414](https://github.com/Arskah/radiodiodj/issues/414)) ([6ef4056](https://github.com/Arskah/radiodiodj/commit/6ef405627a6dab364a4da2d25972bd756a614324))
* **deps:** update dependency prettier to v3.9.7 ([#419](https://github.com/Arskah/radiodiodj/issues/419)) ([0447dfb](https://github.com/Arskah/radiodiodj/commit/0447dfbb47b2f38d9eccf96004e78f407e07c331))
* **deps:** update pnpm to v12.4.2 ([#415](https://github.com/Arskah/radiodiodj/issues/415)) ([da04c62](https://github.com/Arskah/radiodiodj/commit/da04c62cfe280dbad187d23bf81dfad0383d7118))
* **deps:** update vitest monorepo to v5.0.1 ([#413](https://github.com/Arskah/radiodiodj/issues/413)) ([89c3651](https://github.com/Arskah/radiodiodj/commit/89c3651350c8f5286e2089d185d0a31cbcda2a01))
* update funding ([3ba6aa6](https://github.com/Arskah/radiodiodj/commit/3ba6aa681a106bbea92c4b544d1c29b082887966))


### Documentation

* update signing doc ([1d4aac0](https://github.com/Arskah/radiodiodj/commit/1d4aac04b83d1c3bbf94b1afdbfe02839ae92372))
* update signing doc ([ac9713d](https://github.com/Arskah/radiodiodj/commit/ac9713deaf59c4beca69cf8613f5fd583cf48a9e))


### Continuous Integration

* extract building and publishing into release.yml ([#421](https://github.com/Arskah/radiodiodj/issues/421)) ([d69fe51](https://github.com/Arskah/radiodiodj/commit/d69fe5105a6f961e7aeab8f7432c4a09560b669b))
* upload bundles from the platforms that did build ([#420](https://github.com/Arskah/radiodiodj/issues/420)) ([407de0c](https://github.com/Arskah/radiodiodj/commit/407de0c3e8689390b33ca72473e416d27d7c09ed))

## [0.21.1](https://github.com/Arskah/radiodiodj/compare/v0.21.0...v0.21.1) (2026-09-19)


### Bug Fixes

* **ci:** send macOS notarization down the API key route ([#417](https://github.com/Arskah/radiodiodj/issues/417)) ([9d4c001](https://github.com/Arskah/radiodiodj/commit/9d4c001e40a69961e5d42bb16a0b1612c533d72d))

## [0.21.0](https://github.com/Arskah/radiodiodj/compare/v0.20.0...v0.21.0) (2026-09-19)


### Features

* **settings:** pick a theme from an Appearance tab ([#408](https://github.com/Arskah/radiodiodj/issues/408)) ([790389d](https://github.com/Arskah/radiodiodj/commit/790389d28c69fe3718b55a6c3330471575dc9f01))
* **theming:** resolve and paint operator themes ([#407](https://github.com/Arskah/radiodiodj/issues/407)) ([7c5c290](https://github.com/Arskah/radiodiodj/commit/7c5c290cab245ce3b8af679f695a2b3fb55d6b3d))
* **theming:** station name, toolbar logo and record label ([#409](https://github.com/Arskah/radiodiodj/issues/409)) ([d6d548a](https://github.com/Arskah/radiodiodj/commit/d6d548a77fecb7d4fcbaa033775f217a7fc5857b))


### Miscellaneous Chores

* add buy me a coffee ([4bd678c](https://github.com/Arskah/radiodiodj/commit/4bd678c491a23423e1c5e7b4fd4359847e77f18b))
* add GNU GPL v3 license ([f8d02a1](https://github.com/Arskah/radiodiodj/commit/f8d02a1e74eeecffbd4cab28566ef4cdd7f3bcf8))


### Documentation

* **theming:** design record for theming support ([#403](https://github.com/Arskah/radiodiodj/issues/403)) ([e34b3c3](https://github.com/Arskah/radiodiodj/commit/e34b3c358c0c6caaf989379cf2c0454b695c38ef))
* **theming:** design record for theming support ([#405](https://github.com/Arskah/radiodiodj/issues/405)) ([e34b3c3](https://github.com/Arskah/radiodiodj/commit/e34b3c358c0c6caaf989379cf2c0454b695c38ef))


### Code Refactoring

* **styles:** put every colour behind a theme token ([#406](https://github.com/Arskah/radiodiodj/issues/406)) ([408bf51](https://github.com/Arskah/radiodiodj/commit/408bf512b76f24f127030490594926bab6f9743d))


### Continuous Integration

* **macos:** wire Developer ID signing and notarization ([#416](https://github.com/Arskah/radiodiodj/issues/416)) ([a34458c](https://github.com/Arskah/radiodiodj/commit/a34458c74939bc3db0084db23bf664fabb08c487))

## [0.20.0](https://github.com/Arskah/radiodiodj/compare/v0.19.0...v0.20.0) (2026-09-18)


### Features

* **playlist:** airing log, and history moves to the backend ([#402](https://github.com/Arskah/radiodiodj/issues/402)) ([9220b81](https://github.com/Arskah/radiodiodj/commit/9220b81a0ea183ed906cc03d714cd572111d5691))

## [0.19.0](https://github.com/Arskah/radiodiodj/compare/v0.18.0...v0.19.0) (2026-09-18)


### Features

* **audio:** segue handover at Next start on the program bus ([#399](https://github.com/Arskah/radiodiodj/issues/399)) ([8befcf0](https://github.com/Arskah/radiodiodj/commit/8befcf0b22330af5ad6be456e026e30cd307be7b)), closes [#278](https://github.com/Arskah/radiodiodj/issues/278)
* **deck:** live fade-out / fade-to-next transport actions ([#400](https://github.com/Arskah/radiodiodj/issues/400)) ([aee16c7](https://github.com/Arskah/radiodiodj/commit/aee16c73491bdc3787ac0c2c5d2b5a121812e06a)), closes [#280](https://github.com/Arskah/radiodiodj/issues/280)
* **ui:** admin mode — password-lock settings and metadata edits ([#396](https://github.com/Arskah/radiodiodj/issues/396)) ([7382d8a](https://github.com/Arskah/radiodiodj/commit/7382d8a0ec24db312c020fc5d7d295333a8b69db)), closes [#386](https://github.com/Arskah/radiodiodj/issues/386)
* **ui:** overhaul the cue point editor ([#398](https://github.com/Arskah/radiodiodj/issues/398)) ([42981b7](https://github.com/Arskah/radiodiodj/commit/42981b7107a7574b29aed5b332f641e2270ce503))


### Bug Fixes

* don't wait for cue track to load before updating UI on new track ([e0652e3](https://github.com/Arskah/radiodiodj/commit/e0652e359484b9689293dd72b0c6da87eda5e945))
* **ui:** don't let a context menu dismiss itself as it opens ([5400c49](https://github.com/Arskah/radiodiodj/commit/5400c49a9d61038fabbff7cd9075c493ba3f678b))


### Tests

* **e2e:** wait out the metadata dialog's slide-in before clicking ([6f55c4c](https://github.com/Arskah/radiodiodj/commit/6f55c4cf0a6e4a2558c233d0ae7eeee3d70622ad))
* fix flakiness with a timeout ([e61f06b](https://github.com/Arskah/radiodiodj/commit/e61f06b813f90d9855d17b85cafbbdd35f04d6ae))

## [0.18.0](https://github.com/Arskah/radiodiodj/compare/v0.17.0...v0.18.0) (2026-09-16)


### ⚠ BREAKING CHANGES

* **library:** libraries from older versions are rebuilt on first launch. Play counts, metadata edits and waveforms are not carried over; the old database is kept as radiodiodj.legacy-v{N}.bak.db.

### Features

* **audio:** apply per-track cue points on air ([#362](https://github.com/Arskah/radiodiodj/issues/362)) ([f559f22](https://github.com/Arskah/radiodiodj/commit/f559f2212124462b05e288cd2537c2eb083accaa)), closes [#279](https://github.com/Arskah/radiodiodj/issues/279)
* **audio:** apply stored fades as a source envelope ([#363](https://github.com/Arskah/radiodiodj/issues/363)) ([01ed7c5](https://github.com/Arskah/radiodiodj/commit/01ed7c5d8cb6c7c604a2ac4c16ba010b6830af6e)), closes [#279](https://github.com/Arskah/radiodiodj/issues/279)
* **library:** add "Add as next" and "Show in folder" to row menu ([#378](https://github.com/Arskah/radiodiodj/issues/378)) ([392a1e4](https://github.com/Arskah/radiodiodj/commit/392a1e48b863530926faa7ca84b3619120f10890))
* **library:** keep metadata edits across rescans, with opt-in file write-back ([#384](https://github.com/Arskah/radiodiodj/issues/384)) ([d7f835a](https://github.com/Arskah/radiodiodj/commit/d7f835a888315b95c12493a16297133b1dbff455)), closes [#313](https://github.com/Arskah/radiodiodj/issues/313)
* **library:** keep track identity across moves and rescans ([#375](https://github.com/Arskah/radiodiodj/issues/375)) ([#375](https://github.com/Arskah/radiodiodj/issues/375)) ([1b32206](https://github.com/Arskah/radiodiodj/commit/1b322065e7860893cc91d161500ddf7e704d4416)), closes [#373](https://github.com/Arskah/radiodiodj/issues/373)
* **library:** library health view — missing tracks, duplicates and disk changes ([#380](https://github.com/Arskah/radiodiodj/issues/380)) ([de906b3](https://github.com/Arskah/radiodiodj/commit/de906b3e8ab9fd3e041df6cdac186e920524025d))
* **playlist:** give a queued airing cue points of its own ([#367](https://github.com/Arskah/radiodiodj/issues/367)) ([4dd578f](https://github.com/Arskah/radiodiodj/commit/4dd578f7d260039fb14fe52eb2e9a19e1a200bc6))
* **playlist:** show queued air time and time left on air ([#374](https://github.com/Arskah/radiodiodj/issues/374)) ([55d0f0f](https://github.com/Arskah/radiodiodj/commit/55d0f0f37dc112e498cd082bc2eb2858a3628279)), closes [#366](https://github.com/Arskah/radiodiodj/issues/366)
* **ui:** author cue points and report air time ([#365](https://github.com/Arskah/radiodiodj/issues/365)) ([4e0a93c](https://github.com/Arskah/radiodiodj/commit/4e0a93cae22899edf9dcad49fdfb8a71330d3dee))


### Bug Fixes

* **deps:** update rust crate lofty to v0.25.2 ([#382](https://github.com/Arskah/radiodiodj/issues/382)) ([a4aa910](https://github.com/Arskah/radiodiodj/commit/a4aa910e4040bbea69801b929ae84f4304e1962f))
* **health:** show when a library check is runnin ([#391](https://github.com/Arskah/radiodiodj/issues/391)) ([293183f](https://github.com/Arskah/radiodiodj/commit/293183fc25fbf6913096f9baffe31a4ba740ac4d))
* **library:** drop WMA support ([#389](https://github.com/Arskah/radiodiodj/issues/389)) ([378a9db](https://github.com/Arskah/radiodiodj/commit/378a9db464eae83ef2dd141ae51ef24875fc5f6a))
* **library:** include album when grouping possible duplicates ([#393](https://github.com/Arskah/radiodiodj/issues/393)) ([b45697a](https://github.com/Arskah/radiodiodj/commit/b45697afe53e4cb520f33768721e08749eab5804)), closes [#385](https://github.com/Arskah/radiodiodj/issues/385)
* **library:** record files the analysis pass cannot decode ([#390](https://github.com/Arskah/radiodiodj/issues/390)) ([28ede34](https://github.com/Arskah/radiodiodj/commit/28ede34a9f4be4efdb631d5f085166a2c237903d))


### Miscellaneous Chores

* **deps:** update webdriverio monorepo to v9.31.9 ([#392](https://github.com/Arskah/radiodiodj/issues/392)) ([e3041f4](https://github.com/Arskah/radiodiodj/commit/e3041f41f2698823dc1ab9d3df967ce0442623ed))


### Documentation

* **cue-points:** describe the shipped feature ([#369](https://github.com/Arskah/radiodiodj/issues/369)) ([80e8b33](https://github.com/Arskah/radiodiodj/commit/80e8b336fec851fc439153600e10bbf7de8b5810))
* **library:** design the library health view ([#376](https://github.com/Arskah/radiodiodj/issues/376)) ([#379](https://github.com/Arskah/radiodiodj/issues/379)) ([a387ac9](https://github.com/Arskah/radiodiodj/commit/a387ac95192f21c36fee43a8beac31117d856566))


### Build System

* load Vite configs as ESM ([#394](https://github.com/Arskah/radiodiodj/issues/394)) ([35c8046](https://github.com/Arskah/radiodiodj/commit/35c80467808a1d9043d8806c2c9b940414dbe7b4))

## [0.17.0](https://github.com/Arskah/radiodiodj/compare/v0.16.0...v0.17.0) (2026-09-16)


### Features

* **audio:** put every on-air deck on a program bus ([#361](https://github.com/Arskah/radiodiodj/issues/361)) ([20653ef](https://github.com/Arskah/radiodiodj/commit/20653efeaf10010a9eef25428b92afff889df479))
* **playlist:** move the playlist into the backend ([#359](https://github.com/Arskah/radiodiodj/issues/359)) ([5532bc0](https://github.com/Arskah/radiodiodj/commit/5532bc09c056dfe3ef91e04d5a19b9a9efe972d2))


### Bug Fixes

* **deps:** update dependency @tauri-apps/plugin-dialog to v2.7.3 ([#342](https://github.com/Arskah/radiodiodj/issues/342)) ([7ebabb2](https://github.com/Arskah/radiodiodj/commit/7ebabb27eebd9c880e3d0bb74240d5fb7eca078f))
* **deps:** update dependency @tauri-apps/plugin-log to v2.9.1 ([#228](https://github.com/Arskah/radiodiodj/issues/228)) ([88fc3aa](https://github.com/Arskah/radiodiodj/commit/88fc3aa11edd48d51c54070db50ebf0ece0607f8))
* **deps:** update dependency material-symbols to v0.47.2 ([#336](https://github.com/Arskah/radiodiodj/issues/336)) ([9c88d13](https://github.com/Arskah/radiodiodj/commit/9c88d13802872de849afb7b70276ca2e3c94dcc9))
* **deps:** update rust crate lofty to v0.25.1 ([#321](https://github.com/Arskah/radiodiodj/issues/321)) ([923a6ba](https://github.com/Arskah/radiodiodj/commit/923a6bad41df8781c7a0ccd032b6155d40b33da9))
* **deps:** update rust crate log to v0.4.34 ([#328](https://github.com/Arskah/radiodiodj/issues/328)) ([6cd76c3](https://github.com/Arskah/radiodiodj/commit/6cd76c30a10fbf4c7e6838f9e4b94cc6bae6b185))
* **deps:** update rust crate reqwest to v0.13.5 ([#351](https://github.com/Arskah/radiodiodj/issues/351)) ([ca706ad](https://github.com/Arskah/radiodiodj/commit/ca706adaabd3a23a35be4c72a3e528ed609516f8))
* **deps:** update rust crate tauri-plugin-dialog to v2.7.3 ([#343](https://github.com/Arskah/radiodiodj/issues/343)) ([81b003e](https://github.com/Arskah/radiodiodj/commit/81b003ed768efe0dbea0138a2c43cff7a3af982c))
* **deps:** update rust crate tauri-plugin-log to v2.9.1 ([#229](https://github.com/Arskah/radiodiodj/issues/229)) ([461cab4](https://github.com/Arskah/radiodiodj/commit/461cab486fcf25f0a310bb7c65970e8227b55d6a))


### Miscellaneous Chores

* **deps:** lock file maintenance ([#319](https://github.com/Arskah/radiodiodj/issues/319)) ([fdf0156](https://github.com/Arskah/radiodiodj/commit/fdf0156c9064eda20d44c981357364b7426ec774))
* **deps:** update dependency @types/node to v25.9.6 ([#353](https://github.com/Arskah/radiodiodj/issues/353)) ([a7dcf18](https://github.com/Arskah/radiodiodj/commit/a7dcf18e38960fcb2c59812a9ecc8cdc5b30cec0))
* **deps:** update dependency eslint to v10.10.0 ([#331](https://github.com/Arskah/radiodiodj/issues/331)) ([823324b](https://github.com/Arskah/radiodiodj/commit/823324b847666d83352bd89dacfec28715e9a3ed))
* **deps:** update dependency lint-staged to v17.5.1 ([#337](https://github.com/Arskah/radiodiodj/issues/337)) ([4cc5ec4](https://github.com/Arskah/radiodiodj/commit/4cc5ec43a6803b3dad25b54e9b3e7f4b1ae15d0c))
* **deps:** update dependency rust to v1.98.1 ([#326](https://github.com/Arskah/radiodiodj/issues/326)) ([9ce2d6e](https://github.com/Arskah/radiodiodj/commit/9ce2d6efb15f67488738e0e9012a7c5f810cda14))
* **deps:** update dependency svelte to v5.57.0 ([#338](https://github.com/Arskah/radiodiodj/issues/338)) ([05b90f9](https://github.com/Arskah/radiodiodj/commit/05b90f995f6ca1fa21891b3f0eb9d9c218b7ab99))
* **deps:** update dependency tsx to v4.23.13 ([#341](https://github.com/Arskah/radiodiodj/issues/341)) ([d00b90f](https://github.com/Arskah/radiodiodj/commit/d00b90ff4848c9c9267109084dd57c6295edb861))
* **deps:** update dependency typescript-eslint to v8.70.0 ([#332](https://github.com/Arskah/radiodiodj/issues/332)) ([9f978f1](https://github.com/Arskah/radiodiodj/commit/9f978f1173383cf0441409f4a055cdb1a92f12c1))
* **deps:** update dependency vite to v8.3.0 ([#327](https://github.com/Arskah/radiodiodj/issues/327)) ([8d7a1bd](https://github.com/Arskah/radiodiodj/commit/8d7a1bd85b643119434d04a4f137f4ccfa7da23b))
* **deps:** update node.js to v24.21.0 ([#349](https://github.com/Arskah/radiodiodj/issues/349)) ([12e3a34](https://github.com/Arskah/radiodiodj/commit/12e3a341cb3561cd1a3080f55713b79ac9e18fdc))
* **deps:** update pnpm to v12.4.1 ([#347](https://github.com/Arskah/radiodiodj/issues/347)) ([45cbb57](https://github.com/Arskah/radiodiodj/commit/45cbb573fe662d67dbeb42cbe617bd50aacce584))
* **deps:** update pnpm/action-setup action to v6.1.0 ([#348](https://github.com/Arskah/radiodiodj/issues/348)) ([559c9ca](https://github.com/Arskah/radiodiodj/commit/559c9cace1a90447e13e433f7a30d8f07a2e0603))
* **deps:** update webdriverio monorepo ([#333](https://github.com/Arskah/radiodiodj/issues/333)) ([d3ca415](https://github.com/Arskah/radiodiodj/commit/d3ca41588db52edd81d61038bbe7f05ec3064b10))


### Documentation

* **audio:** cue points, program bus, playlist ownership ([#358](https://github.com/Arskah/radiodiodj/issues/358)) ([24d5c4c](https://github.com/Arskah/radiodiodj/commit/24d5c4c8bb6e55a137f2c13cbb8f454556d91188))

## [0.16.0](https://github.com/Arskah/radiodiodj/compare/v0.15.0...v0.16.0) (2026-09-15)


### Features

* **library:** right-click context menu for track rows ([#357](https://github.com/Arskah/radiodiodj/issues/357)) ([4d92f19](https://github.com/Arskah/radiodiodj/commit/4d92f19395681e575fe20c63618695fdfaaba61e)), closes [#314](https://github.com/Arskah/radiodiodj/issues/314)
* **ui:** harden operator UI for broadcast workflow ([#355](https://github.com/Arskah/radiodiodj/issues/355)) ([428e0f5](https://github.com/Arskah/radiodiodj/commit/428e0f57926911c755e1bb0f36077519c9d7434c))


### Bug Fixes

* **deps:** update dependency material-symbols to v0.46.0 ([#320](https://github.com/Arskah/radiodiodj/issues/320)) ([7b0d8bb](https://github.com/Arskah/radiodiodj/commit/7b0d8bb82298d8966e96769c67cedbaf625c7a70))
* **playlist:** shrink track info on hover area ([#352](https://github.com/Arskah/radiodiodj/issues/352)) ([54054fc](https://github.com/Arskah/radiodiodj/commit/54054fc3dfb701456e654948ee15750280c23d39))


### Miscellaneous Chores

* **deps:** update commitlint monorepo to v21.2.2 ([#318](https://github.com/Arskah/radiodiodj/issues/318)) ([7afbd89](https://github.com/Arskah/radiodiodj/commit/7afbd89f13b2ef3eb29dc0eec2808a71871f9fba))
* **deps:** update dependency eslint to v10.9.0 ([#329](https://github.com/Arskah/radiodiodj/issues/329)) ([c1b8da7](https://github.com/Arskah/radiodiodj/commit/c1b8da73f5ed04997ef8aa9f88f1873407ad74f0))
* **deps:** update dependency svelte to v5.56.10 ([#316](https://github.com/Arskah/radiodiodj/issues/316)) ([99fb721](https://github.com/Arskah/radiodiodj/commit/99fb7213639d511104347a8baf9554c0155ad85c))
* **deps:** update dependency svelte-check to v4.7.6 ([#317](https://github.com/Arskah/radiodiodj/issues/317)) ([4a6a329](https://github.com/Arskah/radiodiodj/commit/4a6a329168123d97c89addc706e77ed80ac7d666))
* **deps:** update node.js to v24.20.0 ([#335](https://github.com/Arskah/radiodiodj/issues/335)) ([b5c2de1](https://github.com/Arskah/radiodiodj/commit/b5c2de1e4bb585190279eb4e78d25a287626c9be))
* **deps:** update pnpm to v11.23.0 ([#322](https://github.com/Arskah/radiodiodj/issues/322)) ([b272d4b](https://github.com/Arskah/radiodiodj/commit/b272d4b10e5b0bc865c2a73f9257a4c45b5a13c1))
* **deps:** update pnpm to v12 ([#344](https://github.com/Arskah/radiodiodj/issues/344)) ([a29b57d](https://github.com/Arskah/radiodiodj/commit/a29b57da3c1efc26a431b106be2fcdbc715f97b9))
* **deps:** update pnpm to v12.3.3 ([#346](https://github.com/Arskah/radiodiodj/issues/346)) ([ed813e3](https://github.com/Arskah/radiodiodj/commit/ed813e3615e8b15c2c24620a0d57da7198aade1b))
* **deps:** update vitest monorepo to v4.1.11 ([#325](https://github.com/Arskah/radiodiodj/issues/325)) ([73420bc](https://github.com/Arskah/radiodiodj/commit/73420bcd42cf80ad67d7dd009aba9b038aa2cca9))
* **deps:** update vitest monorepo to v5 ([#345](https://github.com/Arskah/radiodiodj/issues/345)) ([c50edef](https://github.com/Arskah/radiodiodj/commit/c50edef81892a3b466ab0266ca810abdd09a520d))
* **deps:** update webdriverio monorepo ([#324](https://github.com/Arskah/radiodiodj/issues/324)) ([1994878](https://github.com/Arskah/radiodiodj/commit/1994878cfdc8abd46a588fca85e46ae29cc3924d))
* update app-token action ([3771fa4](https://github.com/Arskah/radiodiodj/commit/3771fa489d74512f5288c4b78c3045c3e52fb162))

## [0.15.0](https://github.com/Arskah/radiodiodj/compare/v0.14.0...v0.15.0) (2026-08-14)


### Features

* implement in-app track metadata editing ([#290](https://github.com/Arskah/radiodiodj/issues/290)) ([d5b2483](https://github.com/Arskah/radiodiodj/commit/d5b2483474b2903b7e9beeb5a2a85ad9a1cc2251))
* **settings:** expose hard-coded tuning constants in AppConfig ([#311](https://github.com/Arskah/radiodiodj/issues/311)) ([5353e2a](https://github.com/Arskah/radiodiodj/commit/5353e2af63b2dd0c1c4e491e05e153c226b9ea21))


### Bug Fixes

* **deps:** update dependency material-symbols to v0.45.10 ([#296](https://github.com/Arskah/radiodiodj/issues/296)) ([cdc4fe4](https://github.com/Arskah/radiodiodj/commit/cdc4fe4def0c4818178116cf4f304a971c01a97e))
* **deps:** update rust crate base64 to v0.23.1 ([#302](https://github.com/Arskah/radiodiodj/issues/302)) ([f7a93ea](https://github.com/Arskah/radiodiodj/commit/f7a93eaf3ef1891a0cabb1875771d7dfc8a0c889))
* **deps:** update rust crate lofty to v0.25.0 ([#310](https://github.com/Arskah/radiodiodj/issues/310)) ([2562740](https://github.com/Arskah/radiodiodj/commit/256274022a29e4c69db60e800be994448c4c08dc))
* **deps:** update rust crate rusqlite to v0.40.2 ([#309](https://github.com/Arskah/radiodiodj/issues/309)) ([c7d31be](https://github.com/Arskah/radiodiodj/commit/c7d31be1832035adc3244b27f1b5f111bb38d7c1))


### Miscellaneous Chores

* **deps:** lock file maintenance ([#295](https://github.com/Arskah/radiodiodj/issues/295)) ([de1a4c6](https://github.com/Arskah/radiodiodj/commit/de1a4c61d9fefe4a48c9bf331505eba6558fb5ea))
* **deps:** update dependency @sveltejs/vite-plugin-svelte to v7.3.0 ([#308](https://github.com/Arskah/radiodiodj/issues/308)) ([f39a164](https://github.com/Arskah/radiodiodj/commit/f39a164e1482ebac64e81a9942230754a012e2ba))
* **deps:** update dependency eslint to v10.8.1 ([#305](https://github.com/Arskah/radiodiodj/issues/305)) ([f628522](https://github.com/Arskah/radiodiodj/commit/f62852205756358c39b109fcb353ad2c94a6a1d0))
* **deps:** update dependency jsdom to v30.0.1 ([#291](https://github.com/Arskah/radiodiodj/issues/291)) ([a78518c](https://github.com/Arskah/radiodiodj/commit/a78518ca1e940abab17674cda726fc83891e7305))
* **deps:** update dependency lint-staged to v17.3.0 ([#297](https://github.com/Arskah/radiodiodj/issues/297)) ([ae539f4](https://github.com/Arskah/radiodiodj/commit/ae539f404cc793645b999be137822fac3cc338ae))
* **deps:** update dependency svelte-check to v4.7.5 ([#304](https://github.com/Arskah/radiodiodj/issues/304)) ([e28b176](https://github.com/Arskah/radiodiodj/commit/e28b176e586ff053b31ecf39ae62d8bdfd94098f))
* **deps:** update dependency tsx to v4.23.12 ([#298](https://github.com/Arskah/radiodiodj/issues/298)) ([37babc6](https://github.com/Arskah/radiodiodj/commit/37babc6ec2da70923221dd6c07a03cad83475041))
* **deps:** update dependency typescript-eslint to v8.67.0 ([#301](https://github.com/Arskah/radiodiodj/issues/301)) ([b83cf5d](https://github.com/Arskah/radiodiodj/commit/b83cf5d8c4a692b196d2c2d7b91d5773dc9d59a7))
* **deps:** update dependency vite to v8.2.1 ([#293](https://github.com/Arskah/radiodiodj/issues/293)) ([b06c40b](https://github.com/Arskah/radiodiodj/commit/b06c40bc01b4ba4c75af5db7c9606e6b7dcc5e9c))
* **deps:** update node.js to v24.19.0 ([#294](https://github.com/Arskah/radiodiodj/issues/294)) ([691cf28](https://github.com/Arskah/radiodiodj/commit/691cf28aba0c077f65aa3f05ae14a26d6ab9ae19))
* **deps:** update pnpm to v11.21.0 ([#292](https://github.com/Arskah/radiodiodj/issues/292)) ([51f8a3c](https://github.com/Arskah/radiodiodj/commit/51f8a3c75fd42f9b2052ea52563fa1f6033ee136))
* **deps:** update pnpm/action-setup action to v6.0.10 ([#300](https://github.com/Arskah/radiodiodj/issues/300)) ([15297a0](https://github.com/Arskah/radiodiodj/commit/15297a0f58ff36e0701d2476788b0c1050ac2dc3))
* **deps:** update swatinem/rust-cache action to v2.9.2 ([#303](https://github.com/Arskah/radiodiodj/issues/303)) ([2b056e8](https://github.com/Arskah/radiodiodj/commit/2b056e877d584809e8743c77673362114864f825))
* **deps:** update webdriverio monorepo to v9.30.1 ([#299](https://github.com/Arskah/radiodiodj/issues/299)) ([7c0ceac](https://github.com/Arskah/radiodiodj/commit/7c0ceace191a8d78b088b644b531dfa04d6fbb1c))

## [0.14.0](https://github.com/Arskah/radiodiodj/compare/v0.13.0...v0.14.0) (2026-07-31)


### Features

* vinyl-record app icon + player disc ([#288](https://github.com/Arskah/radiodiodj/issues/288)) ([9704dbe](https://github.com/Arskah/radiodiodj/commit/9704dbe0cd256a5a8ae45b0af7a04f30967a71ac))

## [0.13.0](https://github.com/Arskah/radiodiodj/compare/v0.12.0...v0.13.0) (2026-07-31)


### ⚠ BREAKING CHANGES

* rename app to RadiodioDJ ([#166](https://github.com/Arskah/radiodiodj/issues/166))

### Features

* **deck:** rotating vinyl disc with embedded cover art ([#274](https://github.com/Arskah/radiodiodj/issues/274)) ([9a56653](https://github.com/Arskah/radiodiodj/commit/9a56653f459c19f13ce6db19cab45ef030da88e4))
* **signing:** ad-hoc sign macOS, scaffold Windows + Linux signing ([#285](https://github.com/Arskah/radiodiodj/issues/285)) ([0562884](https://github.com/Arskah/radiodiodj/commit/0562884ec2643f1de3e56fb382181ef8992f5cfd))
* **ui:** convert app to the RadiodioDJ designs ([#272](https://github.com/Arskah/radiodiodj/issues/272)) ([7de1a87](https://github.com/Arskah/radiodiodj/commit/7de1a876713360371a92340ba85b66a1719c4179))


### Bug Fixes

* **deps:** update rust crate base64 to v0.23.0 ([#275](https://github.com/Arskah/radiodiodj/issues/275)) ([afe6a5b](https://github.com/Arskah/radiodiodj/commit/afe6a5b851825373c06adbe1ad164fd280d99e09))
* **playlist:** exclude already-queued track IDs to prevent duplicates ([#277](https://github.com/Arskah/radiodiodj/issues/277)) ([bec1d96](https://github.com/Arskah/radiodiodj/commit/bec1d9621c063d65c7972c6c47ea897169526efa))


### Miscellaneous Chores

* **deps:** update dependency jsdom to v30 ([#266](https://github.com/Arskah/radiodiodj/issues/266)) ([8d326fd](https://github.com/Arskah/radiodiodj/commit/8d326fd777b00450b469fb325e255dceb880c2a5))
* **deps:** update dependency svelte-check to v4.7.4 ([#267](https://github.com/Arskah/radiodiodj/issues/267)) ([18df5a0](https://github.com/Arskah/radiodiodj/commit/18df5a0c709923bbea63f1085bd57c1379b3d334))
* **deps:** update node.js to v24.18.1 ([#268](https://github.com/Arskah/radiodiodj/issues/268)) ([faa74ba](https://github.com/Arskah/radiodiodj/commit/faa74ba92b773eeb1388f9d3de73efdeb4096770))
* point repo URLs at Arskah/radiodiodj ([#167](https://github.com/Arskah/radiodiodj/issues/167)) ([d00e0c2](https://github.com/Arskah/radiodiodj/commit/d00e0c200a68283b01016eb29f877d490eab8b1a))
* rename app to RadiodioDJ ([#166](https://github.com/Arskah/radiodiodj/issues/166)) ([137c901](https://github.com/Arskah/radiodiodj/commit/137c9010c5b8213a03421ceb616e7dff63432bd8))

## [0.12.0](https://github.com/Arskah/diodedj/compare/v0.11.0...v0.12.0) (2026-07-29)


### Features

* **deck:** amplitude curve behind seek bars ([#234](https://github.com/Arskah/diodedj/issues/234)) ([#252](https://github.com/Arskah/diodedj/issues/252)) ([c371fa2](https://github.com/Arskah/diodedj/commit/c371fa288e6595364eb0dc672c27ed67858e745e))
* **deck:** network reconnecting banner + buffering shimmer ([#251](https://github.com/Arskah/diodedj/issues/251)) ([8d81c1f](https://github.com/Arskah/diodedj/commit/8d81c1f39dab0d570d3cf7da4709c23f0e53ade8))
* **deck:** prefetch whole playlist to RAM, bounded by byte cap ([#265](https://github.com/Arskah/diodedj/issues/265)) ([fed8577](https://github.com/Arskah/diodedj/commit/fed857751fd7e5c4237f263659a1dbe74db8714f))


### Bug Fixes

* **deck:** outage recovery — auto-retry reads, no skip after manual next ([#253](https://github.com/Arskah/diodedj/issues/253)) ([c674195](https://github.com/Arskah/diodedj/commit/c67419501fa8d7100c4c67f10156ac8125e04317))
* **deck:** self-healing audio output — launch stream-open failure can't mute the session ([#260](https://github.com/Arskah/diodedj/issues/260)) ([7afc053](https://github.com/Arskah/diodedj/commit/7afc053939bc24d341cd4fb122dd64f10df9ac6a))
* **deps:** update rust crate serde_json to v1.0.151 ([#250](https://github.com/Arskah/diodedj/issues/250)) ([9e9c481](https://github.com/Arskah/diodedj/commit/9e9c4816b3b9feba927cfe75116648d5a8374bbe))
* **deps:** update rust crate tokio to v1.53.1 ([#255](https://github.com/Arskah/diodedj/issues/255)) ([b29a92e](https://github.com/Arskah/diodedj/commit/b29a92e72ac9b310f8f173a614a5b0844ff3fd6d))


### Miscellaneous Chores

* **deps:** lock file maintenance ([#263](https://github.com/Arskah/diodedj/issues/263)) ([5a695d5](https://github.com/Arskah/diodedj/commit/5a695d55ed7bfda0681d96f7e360ad8ae1d733bf))
* **deps:** update actions/checkout action to v7.0.1 ([#254](https://github.com/Arskah/diodedj/issues/254)) ([dab154e](https://github.com/Arskah/diodedj/commit/dab154ee65434784c43e7ab7db30963112554bb9))
* **deps:** update dependency eslint to v10.8.0 ([#264](https://github.com/Arskah/diodedj/issues/264)) ([148bf5f](https://github.com/Arskah/diodedj/commit/148bf5f919c40fa317bd081ac12cbf32dbd10f31))
* **deps:** update dependency lint-staged to v17.2.0 ([#262](https://github.com/Arskah/diodedj/issues/262)) ([a6f5290](https://github.com/Arskah/diodedj/commit/a6f52901be57b092e0a0c65a3bcf355984c32278))
* **deps:** update dependency prettier to v3.9.6 ([#258](https://github.com/Arskah/diodedj/issues/258)) ([18fcdca](https://github.com/Arskah/diodedj/commit/18fcdcaa292825bd4b3e7e124276c8975f7cdd9f))
* **deps:** update dependency svelte to v5.56.8 ([#257](https://github.com/Arskah/diodedj/issues/257)) ([36a36d1](https://github.com/Arskah/diodedj/commit/36a36d1e172d0cba021a9c0b2558b3b906536025))
* **deps:** update dependency typescript-eslint to v8.65.0 ([#256](https://github.com/Arskah/diodedj/issues/256)) ([6492f34](https://github.com/Arskah/diodedj/commit/6492f3445bada391bf55d379e745af41eb34a418))
* **deps:** update pnpm to v11.17.0 ([#249](https://github.com/Arskah/diodedj/issues/249)) ([7bdca48](https://github.com/Arskah/diodedj/commit/7bdca485de43cc6b12985561d5c0aaf96f1fe759))
* **deps:** update webdriverio monorepo to v9.30.0 ([#261](https://github.com/Arskah/diodedj/issues/261)) ([b751c7d](https://github.com/Arskah/diodedj/commit/b751c7d6f832418771228bc6833947a6810870fb))


### Code Refactoring

* add isStrictNever to force handling all cases in switch ([01505b1](https://github.com/Arskah/diodedj/commit/01505b12c40276c0a06a12d8ed4695a56652e1b5))

## [0.11.0](https://github.com/Arskah/diodedj/compare/v0.10.2...v0.11.0) (2026-07-22)


### ⚠ BREAKING CHANGES

* rm log breaking .app ([#121](https://github.com/Arskah/diodedj/issues/121))

### Features

* **audio:** network-resilient audio player (SMB/NFS share hardening) ([#241](https://github.com/Arskah/diodedj/issues/241)) ([760189f](https://github.com/Arskah/diodedj/commit/760189fbb885702cb8a37e79ba21c081d40ca7c4))


### Bug Fixes

* **deps:** update dependency @tauri-apps/plugin-dialog to v2.7.2 ([#243](https://github.com/Arskah/diodedj/issues/243)) ([0d68075](https://github.com/Arskah/diodedj/commit/0d68075032da149a14160c08bbc7bf4a4a4af9ee))
* **deps:** update rust crate anyhow to v1.0.104 ([#245](https://github.com/Arskah/diodedj/issues/245)) ([42272ee](https://github.com/Arskah/diodedj/commit/42272ee70fb34105a355181b5f9d928ffda50ea5))
* **deps:** update rust crate serde to v1.0.229 ([#246](https://github.com/Arskah/diodedj/issues/246)) ([9f47c6f](https://github.com/Arskah/diodedj/commit/9f47c6f27b7a9b30063fdf45a5fe08b0b090b800))
* **deps:** update rust crate tauri-plugin-dialog to v2.7.2 ([#244](https://github.com/Arskah/diodedj/issues/244)) ([ab86935](https://github.com/Arskah/diodedj/commit/ab86935d2e6c2d109fbe9c1ad462e4318827f1ac))
* **deps:** update rust crate tokio to v1.53.0 ([#232](https://github.com/Arskah/diodedj/issues/232)) ([c078fcf](https://github.com/Arskah/diodedj/commit/c078fcff27387072d3638242f0dc265c3f789236))
* rm log breaking .app ([#121](https://github.com/Arskah/diodedj/issues/121)) ([a04e08a](https://github.com/Arskah/diodedj/commit/a04e08acd349f18b3d35275c91f3830be0f38c60))


### Miscellaneous Chores

* add AGENTS.md ([5a65be0](https://github.com/Arskah/diodedj/commit/5a65be0ab0e487b521af6a7fcd4c65b6001d1e61))
* **deps:** lock file maintenance ([#247](https://github.com/Arskah/diodedj/issues/247)) ([79bc39a](https://github.com/Arskah/diodedj/commit/79bc39ae65486987bf12854761f41d4ef8ddd184))
* **deps:** update actions/setup-node action to v7 ([#230](https://github.com/Arskah/diodedj/issues/230)) ([eb025ca](https://github.com/Arskah/diodedj/commit/eb025cab08df393914f39426229f1f58142031ed))
* **deps:** update dependency @commitlint/cli to v21.2.1 ([#219](https://github.com/Arskah/diodedj/issues/219)) ([a8b3392](https://github.com/Arskah/diodedj/commit/a8b33922ab5c8238ab0d7f39e411941ec065bc37))
* **deps:** update dependency @sveltejs/vite-plugin-svelte to v7.2.0 ([#215](https://github.com/Arskah/diodedj/issues/215)) ([56c2491](https://github.com/Arskah/diodedj/commit/56c24914142768e56875ce8f5e7c5db4e21067c8))
* **deps:** update dependency @types/node to v25.9.5 ([#218](https://github.com/Arskah/diodedj/issues/218)) ([99b9dc5](https://github.com/Arskah/diodedj/commit/99b9dc5dd5b426de5af5522fec160f5fe26fcabb))
* **deps:** update dependency eslint to v10.7.0 ([#224](https://github.com/Arskah/diodedj/issues/224)) ([003612b](https://github.com/Arskah/diodedj/commit/003612bf543a51a443ab84a3a81c1d3bc75a9c61))
* **deps:** update dependency lint-staged to v17.1.0 ([#242](https://github.com/Arskah/diodedj/issues/242)) ([406dc2e](https://github.com/Arskah/diodedj/commit/406dc2e26a5a3c6fe6526e2e7fb76ce9329488d0))
* **deps:** update dependency prettier to v3.9.5 ([#223](https://github.com/Arskah/diodedj/issues/223)) ([20f727f](https://github.com/Arskah/diodedj/commit/20f727fe6d3d1428f6adca8fd642b17e9e1dd695))
* **deps:** update dependency rust to v1.97.1 ([#222](https://github.com/Arskah/diodedj/issues/222)) ([c7ca812](https://github.com/Arskah/diodedj/commit/c7ca8121df1c738fcbb1d059e6c75ee4fae63454))
* **deps:** update dependency svelte to v5.56.6 ([#231](https://github.com/Arskah/diodedj/issues/231)) ([7304c5e](https://github.com/Arskah/diodedj/commit/7304c5e9cd5354effd3653ae74241350dfad1e27))
* **deps:** update dependency svelte-check to v4.7.3 ([#217](https://github.com/Arskah/diodedj/issues/217)) ([96e4eb6](https://github.com/Arskah/diodedj/commit/96e4eb6ae1b66d0cb7676f0367cd5f19695db435))
* **deps:** update dependency tsx to v4.23.1 ([#226](https://github.com/Arskah/diodedj/issues/226)) ([fbd0fcb](https://github.com/Arskah/diodedj/commit/fbd0fcbd8c3eae3dad73040a88d1c19291e39ad1))
* **deps:** update dependency typescript-eslint to v8.64.0 ([#216](https://github.com/Arskah/diodedj/issues/216)) ([d0764e2](https://github.com/Arskah/diodedj/commit/d0764e2fdcb75105d04a1e6978944c75b4eba4f1))
* **deps:** update dependency vite to v8.1.5 ([#221](https://github.com/Arskah/diodedj/issues/221)) ([9e2cd0b](https://github.com/Arskah/diodedj/commit/9e2cd0bdec706c076e23e4cee5bda4db5b284ac6))
* **deps:** update pnpm to v11.15.0 ([#213](https://github.com/Arskah/diodedj/issues/213)) ([5b94a7b](https://github.com/Arskah/diodedj/commit/5b94a7bc5154df6ac07b2d122489ff11f1b2c6f8))
* **deps:** update vitest monorepo to v4.1.10 ([#214](https://github.com/Arskah/diodedj/issues/214)) ([5f9184a](https://github.com/Arskah/diodedj/commit/5f9184aceffdbf00c44e0d38aeb3a972584d3a5f))
* sort gitignore ([791ce44](https://github.com/Arskah/diodedj/commit/791ce44bf87a7a421bfa2f0fdd1c6815ca6275f7))

## [0.10.2](https://github.com/Arskah/diodedj/compare/v0.10.1...v0.10.2) (2026-07-07)


### Bug Fixes

* **deps:** update rust crate anyhow to v1.0.103 ([#200](https://github.com/Arskah/diodedj/issues/200)) ([c942e51](https://github.com/Arskah/diodedj/commit/c942e5152ef0a4f12eb2fa18748c6f505e20ff35))
* **deps:** update rust crate chrono to v0.4.45 ([#186](https://github.com/Arskah/diodedj/issues/186)) ([91f9e70](https://github.com/Arskah/diodedj/commit/91f9e705a5ab77f6bccb0d53dcdd49c25ce2251e))
* **deps:** update rust crate hmac to v0.13.0 ([#147](https://github.com/Arskah/diodedj/issues/147)) ([198b0fe](https://github.com/Arskah/diodedj/commit/198b0fe8e6290996a71d3b1a332650b4dc49f2af))
* **deps:** update rust crate log to v0.4.33 ([#169](https://github.com/Arskah/diodedj/issues/169)) ([fa68c2b](https://github.com/Arskah/diodedj/commit/fa68c2b07ff4a3d8c8116cd2485a4ed36ea54d4c))
* **deps:** update rust crate reqwest to v0.13.4 ([#170](https://github.com/Arskah/diodedj/issues/170)) ([2c2a02a](https://github.com/Arskah/diodedj/commit/2c2a02ad50cd9dbfdcbd6eda49536316487e0de3))
* **deps:** update rust crate rusqlite to v0.40.1 ([#172](https://github.com/Arskah/diodedj/issues/172)) ([dde6144](https://github.com/Arskah/diodedj/commit/dde61449b3c2ac1c0871a9f39f60c841f421e776))
* **deps:** update rust crate tauri to v2.11.4 ([#195](https://github.com/Arskah/diodedj/issues/195)) ([88b72f2](https://github.com/Arskah/diodedj/commit/88b72f2d61c42c38791463317369308071784f44))
* **deps:** update rust crate tauri to v2.11.5 ([#209](https://github.com/Arskah/diodedj/issues/209)) ([6085fc0](https://github.com/Arskah/diodedj/commit/6085fc0e87786187bf56f5cfde4bae5d4f4dce89))
* **deps:** update tauri monorepo ([#196](https://github.com/Arskah/diodedj/issues/196)) ([4da0413](https://github.com/Arskah/diodedj/commit/4da041367c23e553fe74f663785d7e5b55b28e5e))


### Miscellaneous Chores

* **deps:** audit fixes ([06554e5](https://github.com/Arskah/diodedj/commit/06554e5d90447b0f58880ada70fe6287f2c86a53))
* **deps:** lock file maintenance ([#176](https://github.com/Arskah/diodedj/issues/176)) ([a73dd7f](https://github.com/Arskah/diodedj/commit/a73dd7f11acbd94525cda76bb5192865058f7893))
* **deps:** lock file maintenance ([#212](https://github.com/Arskah/diodedj/issues/212)) ([9cb1c1e](https://github.com/Arskah/diodedj/commit/9cb1c1e2307c408aa7133590728a171be739d1eb))
* **deps:** pin dependencies ([#164](https://github.com/Arskah/diodedj/issues/164)) ([636f2d9](https://github.com/Arskah/diodedj/commit/636f2d9abde0c6d0abf7a45eb337f8a0d6419fde))
* **deps:** pin dependencies ([#205](https://github.com/Arskah/diodedj/issues/205)) ([5c13a6f](https://github.com/Arskah/diodedj/commit/5c13a6fd648073ed93feb64a17a2f0e9e0748a72))
* **deps:** update actions/checkout action to v7 ([#197](https://github.com/Arskah/diodedj/issues/197)) ([5c252eb](https://github.com/Arskah/diodedj/commit/5c252eb286d059c0fcfc9c56bcc3223cf038abee))
* **deps:** update commitlint monorepo to v21.1.0 ([#177](https://github.com/Arskah/diodedj/issues/177)) ([06540ba](https://github.com/Arskah/diodedj/commit/06540bae1c7f958aefa2839f939e4dd3d9d67fcb))
* **deps:** update commitlint monorepo to v21.2.0 ([#206](https://github.com/Arskah/diodedj/issues/206)) ([5e16a09](https://github.com/Arskah/diodedj/commit/5e16a09dabd20bea1491a2ac03410cde0bd82456))
* **deps:** update dependency @types/node to v25.9.4 ([#187](https://github.com/Arskah/diodedj/issues/187)) ([218e7ed](https://github.com/Arskah/diodedj/commit/218e7edddb60c9ca058bf80a1e3c2cd70252ef30))
* **deps:** update dependency eslint to v10.6.0 ([#178](https://github.com/Arskah/diodedj/issues/178)) ([7a216f6](https://github.com/Arskah/diodedj/commit/7a216f69fd9ea95af28fe12ccce5f8b13d581877))
* **deps:** update dependency lint-staged to v17.0.8 ([#179](https://github.com/Arskah/diodedj/issues/179)) ([02b9d36](https://github.com/Arskah/diodedj/commit/02b9d362cc5fdb166798d7ce23aa6c8cbabdc9bc))
* **deps:** update dependency prettier to v3.9.4 ([#190](https://github.com/Arskah/diodedj/issues/190)) ([ba69a16](https://github.com/Arskah/diodedj/commit/ba69a16ad0d01f2aa37860ce07bb1fa0d4ef5979))
* **deps:** update dependency prettier-plugin-svelte to v4.1.1 ([#183](https://github.com/Arskah/diodedj/issues/183)) ([da19b0d](https://github.com/Arskah/diodedj/commit/da19b0dcfe10a5a9c5ceec61a6f399bf1a4a19c7))
* **deps:** update dependency rust to v1.96.1 ([#175](https://github.com/Arskah/diodedj/issues/175)) ([3662f6b](https://github.com/Arskah/diodedj/commit/3662f6b65ce8d20dc8204bf874d58d352e97a76f))
* **deps:** update dependency svelte to v5.56.4 ([#174](https://github.com/Arskah/diodedj/issues/174)) ([a44caff](https://github.com/Arskah/diodedj/commit/a44caff7d70192a4258325b63236cb2bb2b31282))
* **deps:** update dependency svelte-check to v4.7.1 ([#184](https://github.com/Arskah/diodedj/issues/184)) ([df58317](https://github.com/Arskah/diodedj/commit/df5831703490af947eddf27d97052470d4d8e5de))
* **deps:** update dependency tsx to v4.22.4 ([#180](https://github.com/Arskah/diodedj/issues/180)) ([4d5f5da](https://github.com/Arskah/diodedj/commit/4d5f5daf5740df9540dc99d2643b4872d20191ab))
* **deps:** update dependency tsx to v4.23.0 ([#211](https://github.com/Arskah/diodedj/issues/211)) ([8cfedec](https://github.com/Arskah/diodedj/commit/8cfedec819e639043f7913c55e7e1ebeb77b1170))
* **deps:** update dependency typescript-eslint to v8.62.0 ([#171](https://github.com/Arskah/diodedj/issues/171)) ([1dfb5d5](https://github.com/Arskah/diodedj/commit/1dfb5d55a3c69de25db3f7b2505a28ba520f5bfd))
* **deps:** update dependency typescript-eslint to v8.62.1 ([#203](https://github.com/Arskah/diodedj/issues/203)) ([384cad9](https://github.com/Arskah/diodedj/commit/384cad937e24d157bca80fbc148ce085a53a01eb))
* **deps:** update dependency vite to v8.0.16 [security] ([#191](https://github.com/Arskah/diodedj/issues/191)) ([4bcd720](https://github.com/Arskah/diodedj/commit/4bcd72078376827848f941df51715c909c857de7))
* **deps:** update dependency vite to v8.1.0 ([#202](https://github.com/Arskah/diodedj/issues/202)) ([4a1afe6](https://github.com/Arskah/diodedj/commit/4a1afe6b04cd27e69696c6189b01aa0e456c018c))
* **deps:** update dependency vite to v8.1.1 ([#207](https://github.com/Arskah/diodedj/issues/207)) ([7452a27](https://github.com/Arskah/diodedj/commit/7452a273feea498e2fb6d61fcd77c9b9008e92d1))
* **deps:** update dependency vite to v8.1.2 ([#208](https://github.com/Arskah/diodedj/issues/208)) ([bc2f6e3](https://github.com/Arskah/diodedj/commit/bc2f6e39f7cddad5568088c7825e8cc04d8dca5a))
* **deps:** update dependency vite to v8.1.3 ([#210](https://github.com/Arskah/diodedj/issues/210)) ([24ddbc1](https://github.com/Arskah/diodedj/commit/24ddbc15e1090828faa9ae90dea5ddfb50c6ac61))
* **deps:** update node.js to v24.18.0 ([#193](https://github.com/Arskah/diodedj/issues/193)) ([a449df3](https://github.com/Arskah/diodedj/commit/a449df39b3f3175484dcc0dc9b3737afe9913d1a))
* **deps:** update pnpm to v11.9.0 ([#168](https://github.com/Arskah/diodedj/issues/168)) ([8baab90](https://github.com/Arskah/diodedj/commit/8baab90f40917ffe674c384e26f71a1c1939d55d))
* **deps:** update pnpm/action-setup action to v6.0.9 ([#192](https://github.com/Arskah/diodedj/issues/192)) ([fe47d71](https://github.com/Arskah/diodedj/commit/fe47d717c3295321aaf047436d2bcca302684095))
* **deps:** update rust crate tauri-build to v2.6.3 ([#194](https://github.com/Arskah/diodedj/issues/194)) ([101e393](https://github.com/Arskah/diodedj/commit/101e393832c3c548fb6832cd819433c8263ff956))
* **deps:** update tauri-apps/tauri-action action to v1 ([#204](https://github.com/Arskah/diodedj/issues/204)) ([3cc6c6e](https://github.com/Arskah/diodedj/commit/3cc6c6ee5f2d1ae5792ddf5862459a2963d56ace))
* **deps:** update vitest monorepo to v4.1.9 ([#182](https://github.com/Arskah/diodedj/issues/182)) ([04e2c48](https://github.com/Arskah/diodedj/commit/04e2c4863ec6dfcddfb4e7ae882037a56251d12c))
* **deps:** update webdriverio monorepo to v9.29.1 ([#173](https://github.com/Arskah/diodedj/issues/173)) ([7733705](https://github.com/Arskah/diodedj/commit/773370542c4271016bced5793c084ed458ca084a))

## [0.10.1](https://github.com/Arskah/diodedj/compare/v0.10.0...v0.10.1) (2026-05-26)


### Bug Fixes

* **deps:** update rust crate serde_json to v1.0.150 ([#161](https://github.com/Arskah/diodedj/issues/161)) ([c7c7709](https://github.com/Arskah/diodedj/commit/c7c77095d3b343c3e175f2c8aa7711b37f1d5c99))


### Miscellaneous Chores

* cleanup pnpm settings ([0985f71](https://github.com/Arskah/diodedj/commit/0985f71c67472dbfb9e1bfc5f929c27726448873))
* **deps:** lock file maintenance ([#149](https://github.com/Arskah/diodedj/issues/149)) ([0a83c41](https://github.com/Arskah/diodedj/commit/0a83c413061c796874ea19b6cc01b70d15269124))
* **deps:** update dependency @types/node to v25.8.0 ([#128](https://github.com/Arskah/diodedj/issues/128)) ([9ed35a6](https://github.com/Arskah/diodedj/commit/9ed35a63f07850161f210ebb96792f4266222794))
* **deps:** update dependency @types/node to v25.9.1 ([#154](https://github.com/Arskah/diodedj/issues/154)) ([471f3e7](https://github.com/Arskah/diodedj/commit/471f3e79ab882dfcfdb0f39ec184be0e635ac053))
* **deps:** update dependency prettier-plugin-svelte to v4 ([#159](https://github.com/Arskah/diodedj/issues/159)) ([ef12b98](https://github.com/Arskah/diodedj/commit/ef12b98fde4167d3a365c09cdb065babe66b8341))
* **deps:** update dependency svelte to v5.55.9 ([#155](https://github.com/Arskah/diodedj/issues/155)) ([6c08a8b](https://github.com/Arskah/diodedj/commit/6c08a8b8c0eb8d2ca13dcbca0b0a96697af7a967))
* **deps:** update dependency tsx to v4.22.3 ([#151](https://github.com/Arskah/diodedj/issues/151)) ([055dcc6](https://github.com/Arskah/diodedj/commit/055dcc6b61e030fc8c441051aa53b1d0e15225a1))
* **deps:** update dependency typescript-eslint to v8.59.4 ([#156](https://github.com/Arskah/diodedj/issues/156)) ([a90a7a7](https://github.com/Arskah/diodedj/commit/a90a7a7155135989cecbd0f0acd55452e7bf6e7a))
* **deps:** update dependency vite to v8.0.14 ([#160](https://github.com/Arskah/diodedj/issues/160)) ([864caf2](https://github.com/Arskah/diodedj/commit/864caf25abf93869a011a1a4048ca3ee5cdd5703))
* **deps:** update node.js to v24.16.0 ([#158](https://github.com/Arskah/diodedj/issues/158)) ([19a9930](https://github.com/Arskah/diodedj/commit/19a99303783f523e3220d9396efebbb41c6e73ad))
* **deps:** update pnpm to v11.2.2 ([#153](https://github.com/Arskah/diodedj/issues/153)) ([0c35b24](https://github.com/Arskah/diodedj/commit/0c35b24f2ce904f5a158d26d3abb4980260c5759))
* **deps:** update vitest monorepo to v4.1.7 ([#157](https://github.com/Arskah/diodedj/issues/157)) ([ad31abf](https://github.com/Arskah/diodedj/commit/ad31abfaa77fc1685684c34c31dd2c85cc16e46c))
* fix audit issues ([2fa9be1](https://github.com/Arskah/diodedj/commit/2fa9be129bab825848ce9c4f1ab5f4f454044be3))


### Documentation

* add CONTEXT.md ubiquitous language glossary ([9501e61](https://github.com/Arskah/diodedj/commit/9501e61084edb7eb802c68e6ab7ad0eafab7ad21))


### Code Refactoring

* align codebase with CONTEXT.md ubiquitous language ([#163](https://github.com/Arskah/diodedj/issues/163)) ([77e3c9c](https://github.com/Arskah/diodedj/commit/77e3c9cdc94ee7048b6c35c004302969ec666262))

## [0.10.0](https://github.com/Arskah/diodedj/compare/v0.9.1...v0.10.0) (2026-05-19)


### Features

* expose currently playing track via webhook + file output ([#146](https://github.com/Arskah/diodedj/issues/146)) ([d48a39f](https://github.com/Arskah/diodedj/commit/d48a39f3b9e2be9ed4fb9ae1ccfdd15b5f464902))
* stop marker halts auto-play when reached ([#123](https://github.com/Arskah/diodedj/issues/123)) ([777d0aa](https://github.com/Arskah/diodedj/commit/777d0aa0bc8f4c5c8da26e6bf2c30fb58752d5d8))


### Bug Fixes

* **deps:** update tauri monorepo ([#144](https://github.com/Arskah/diodedj/issues/144)) ([7706871](https://github.com/Arskah/diodedj/commit/77068715e2ff50626fd7a50f423bbdbb34753da9))


### Miscellaneous Chores

* **a11y:** add ARIA roles and labels for e2e selectors ([#131](https://github.com/Arskah/diodedj/issues/131)) ([f598b8c](https://github.com/Arskah/diodedj/commit/f598b8c127e401027d6f706bb1ca7a6634a27bc1))
* **deps:** pin node.js ([#145](https://github.com/Arskah/diodedj/issues/145)) ([1d791b2](https://github.com/Arskah/diodedj/commit/1d791b2b7c4b3714a3b766dc33c4373d14cb48d1))
* **deps:** update actions/download-artifact action to v8 ([#135](https://github.com/Arskah/diodedj/issues/135)) ([2d1e3ad](https://github.com/Arskah/diodedj/commit/2d1e3adfbc6d0ff32a872f8053025cce47cdaccf))
* **deps:** update commitlint monorepo to v21.0.1 ([#139](https://github.com/Arskah/diodedj/issues/139)) ([92a3e95](https://github.com/Arskah/diodedj/commit/92a3e95d8ed35f491cc0502575f85c9f228bbd0e))
* **deps:** update dependency eslint to v10.4.0 ([#142](https://github.com/Arskah/diodedj/issues/142)) ([556a36f](https://github.com/Arskah/diodedj/commit/556a36f8b5e13c19f60adf23e0e52d1d13a7430b))
* **deps:** update dependency lint-staged to v17.0.5 ([#143](https://github.com/Arskah/diodedj/issues/143)) ([0f6814b](https://github.com/Arskah/diodedj/commit/0f6814b5ac4e9d76aaefa475e7624e9d393fa126))
* **deps:** update dependency svelte to v5.55.7 [security] ([#133](https://github.com/Arskah/diodedj/issues/133)) ([d29e69b](https://github.com/Arskah/diodedj/commit/d29e69b2548e4b6b812e1bf5e3472a698c603797))
* **deps:** update dependency tsx to v4.22.0 ([#140](https://github.com/Arskah/diodedj/issues/140)) ([0f555fa](https://github.com/Arskah/diodedj/commit/0f555fa472cb3f5313b16cff4638b7a7de9ab25a))
* **deps:** update dependency typescript-eslint to v8.59.3 ([#127](https://github.com/Arskah/diodedj/issues/127)) ([67a0c3f](https://github.com/Arskah/diodedj/commit/67a0c3f7f2328f0febb67d30da741811f2675ce0))
* **deps:** update dependency vite to v8.0.13 ([#141](https://github.com/Arskah/diodedj/issues/141)) ([61c6182](https://github.com/Arskah/diodedj/commit/61c61827949ed795b2fd9be47c891b773563799e))
* **deps:** update pnpm to v11.1.0 ([#129](https://github.com/Arskah/diodedj/issues/129)) ([c0282bc](https://github.com/Arskah/diodedj/commit/c0282bc7828d4076b0da01bf9afddecea80ea3fe))
* **deps:** update pnpm to v11.1.2 ([#137](https://github.com/Arskah/diodedj/issues/137)) ([414f861](https://github.com/Arskah/diodedj/commit/414f8610a96e6a647bbf3d736c4b997bf25ab952))
* **deps:** update pnpm/action-setup action to v6.0.8 ([#138](https://github.com/Arskah/diodedj/issues/138)) ([6bad28f](https://github.com/Arskah/diodedj/commit/6bad28fc3ac46ce8df82efc1b0faab9053e23f05))


### Tests

* **e2e:** add Docker harness for local Linux e2e ([3c989e9](https://github.com/Arskah/diodedj/commit/3c989e98cfd311e8f53bd118db2317682494bd5b))
* **e2e:** port smoke suite to tauri-driver + WebdriverIO ([#132](https://github.com/Arskah/diodedj/issues/132)) ([4a0e649](https://github.com/Arskah/diodedj/commit/4a0e649ebb4a097e154be185d7277828f1e72540))

## [0.9.1](https://github.com/Arskah/diodedj/compare/v0.9.0...v0.9.1) (2026-05-14)


### Bug Fixes

* **playlist:** restore drag-and-drop reorder ([#124](https://github.com/Arskah/diodedj/issues/124)) ([6a762b5](https://github.com/Arskah/diodedj/commit/6a762b56c551b4d64dc3ab7f8fb74c5d9e84c88a))

## [0.9.0](https://github.com/Arskah/diodedj/compare/v0.8.1...v0.9.0) (2026-05-14)


### Features

* **log:** wire tauri-plugin-log for unified logging ([#118](https://github.com/Arskah/diodedj/issues/118)) ([39c1bf5](https://github.com/Arskah/diodedj/commit/39c1bf514bb2e5a3d6bea969ce9f273334ef2807)), closes [#79](https://github.com/Arskah/diodedj/issues/79)
* persist window size/position via tauri-plugin-window-state ([#119](https://github.com/Arskah/diodedj/issues/119)) ([55760b1](https://github.com/Arskah/diodedj/commit/55760b1c17b8fd03bde1c753426fb360bb26b1fa)), closes [#78](https://github.com/Arskah/diodedj/issues/78)


### Bug Fixes

* **deps:** update rust crate tauri to v2.11.1 [security] ([#106](https://github.com/Arskah/diodedj/issues/106)) ([1b5ec8e](https://github.com/Arskah/diodedj/commit/1b5ec8e1f6a0490f89b77d14b611eda82bdf6f58))
* **deps:** update rust crate tokio to v1.52.3 ([#115](https://github.com/Arskah/diodedj/issues/115)) ([020f891](https://github.com/Arskah/diodedj/commit/020f8919ca4b35cc54d1dd2f92434e47cb34bc56))
* fairer commercial selection in auto-playlist ([#102](https://github.com/Arskah/diodedj/issues/102)) ([1bcd205](https://github.com/Arskah/diodedj/commit/1bcd205bb75cfc7a8bfc7dc3ab4df4bd6f71e6f6))


### Miscellaneous Chores

* **deps:** lock file maintenance ([#114](https://github.com/Arskah/diodedj/issues/114)) ([5659040](https://github.com/Arskah/diodedj/commit/5659040d66f51886f655ecf9af566dfcb541e915))
* **deps:** update commitlint monorepo to v21 ([#116](https://github.com/Arskah/diodedj/issues/116)) ([d1816dd](https://github.com/Arskah/diodedj/commit/d1816ddede4d5abcdb0ac66e8e2272619a10fb70))
* **deps:** update dependency @sveltejs/vite-plugin-svelte to v7.1.2 ([#108](https://github.com/Arskah/diodedj/issues/108)) ([83e14f1](https://github.com/Arskah/diodedj/commit/83e14f17acbb1dc23fa75324e21d98f7faa629fb))
* **deps:** update dependency @types/node to v25.6.2 ([#113](https://github.com/Arskah/diodedj/issues/113)) ([3083daa](https://github.com/Arskah/diodedj/commit/3083daab7b67f80e29c8edcc3dbf076f96afc9ad))
* **deps:** update dependency lint-staged to v17 ([#110](https://github.com/Arskah/diodedj/issues/110)) ([f778746](https://github.com/Arskah/diodedj/commit/f778746232ff42aecd23965ec2b6233e840573cb))
* **deps:** update dependency prettier-plugin-svelte to v3.5.2 ([#120](https://github.com/Arskah/diodedj/issues/120)) ([d74a0e7](https://github.com/Arskah/diodedj/commit/d74a0e7974a5fe808fcf1aac215e62c8b1869d59))
* **deps:** update dependency svelte-check to v4.4.8 ([#107](https://github.com/Arskah/diodedj/issues/107)) ([c4d1fb0](https://github.com/Arskah/diodedj/commit/c4d1fb0afebbbe100575ba432ac21ddc68631b23))
* **deps:** update dependency typescript-eslint to v8.59.2 ([#105](https://github.com/Arskah/diodedj/issues/105)) ([ed36b88](https://github.com/Arskah/diodedj/commit/ed36b884aef421ac83d8b7ce783ffaf349b6722b))
* **deps:** update dependency vite to v8.0.12 ([#112](https://github.com/Arskah/diodedj/issues/112)) ([e764ab1](https://github.com/Arskah/diodedj/commit/e764ab152ea4a6aa3b6085dcfee7cd271dea6dbd))
* **deps:** update pnpm to v11 ([#104](https://github.com/Arskah/diodedj/issues/104)) ([526044c](https://github.com/Arskah/diodedj/commit/526044cbe23eee2cb5a6a2c25a2f2e6aa9ea4449))
* **deps:** update pnpm/action-setup action to v6.0.7 ([#117](https://github.com/Arskah/diodedj/issues/117)) ([923195f](https://github.com/Arskah/diodedj/commit/923195f4a46f833f530b30f1ab30e13d161f44d8))
* **deps:** update tauri monorepo ([#109](https://github.com/Arskah/diodedj/issues/109)) ([f1b4572](https://github.com/Arskah/diodedj/commit/f1b457223f0d8bcdbbbee93c5261ce267d31b1c7))
* **deps:** update vitest monorepo to v4.1.6 ([#122](https://github.com/Arskah/diodedj/issues/122)) ([8a1c4c5](https://github.com/Arskah/diodedj/commit/8a1c4c51f1ae47b60510133116aff4db2bbcf380))

## [0.8.1](https://github.com/Arskah/diodedj/compare/v0.8.0...v0.8.1) (2026-05-05)


### Bug Fixes

* **deps:** update dependency @tauri-apps/plugin-dialog to v2.7.1 ([#96](https://github.com/Arskah/diodedj/issues/96)) ([0f5db32](https://github.com/Arskah/diodedj/commit/0f5db3288a0495f5f637350e5b723dfbc58b5b16))
* increase cpal buffer size ([3ced281](https://github.com/Arskah/diodedj/commit/3ced2813c0a05cc9103532e14f59b7baab0e1dd3))
* replace debounced save with throttled ([339e342](https://github.com/Arskah/diodedj/commit/339e342eaad08e88e8dc4f101f967bf17c551295))


### Miscellaneous Chores

* add extension recommendations ([8e60a9d](https://github.com/Arskah/diodedj/commit/8e60a9df9f667fb9b9fb1e1fe84e76fcce3b5a37))
* **deps:** update pnpm to v10.33.3 ([#100](https://github.com/Arskah/diodedj/issues/100)) ([5675294](https://github.com/Arskah/diodedj/commit/5675294d186658eead7c8dd5e76f9a9e263795c2))
* **deps:** update pnpm/action-setup action to v6.0.5 ([#98](https://github.com/Arskah/diodedj/issues/98)) ([214521d](https://github.com/Arskah/diodedj/commit/214521d7db92da4a903ba051d26133546f4694b3))
* setup .tool-versions ([461f91f](https://github.com/Arskah/diodedj/commit/461f91f684c8f5bc270c0ad74742b6e55b757323))

## [0.8.0](https://github.com/Arskah/diodedj/compare/v0.7.2...v0.8.0) (2026-05-05)


### Features

* **audio:** add device enumeration + per-deck device config ([a305ba4](https://github.com/Arskah/diodedj/commit/a305ba44d03d0de21d024bbf31256f148fe2885e))
* **audio:** cue deck backend ([#90](https://github.com/Arskah/diodedj/issues/90)) ([97dda0d](https://github.com/Arskah/diodedj/commit/97dda0def8461394cc85dd01a37a74d95eed5337))
* **audio:** cue deck backend (commands, events, lazy spawn) ([97dda0d](https://github.com/Arskah/diodedj/commit/97dda0def8461394cc85dd01a37a74d95eed5337))
* **audio:** cue deck UI + audio device settings ([#91](https://github.com/Arskah/diodedj/issues/91)) ([cd02131](https://github.com/Arskah/diodedj/commit/cd02131f8bf5655079d46fabc5a5149dd17cbb96))
* **audio:** device enumeration + per-deck device config ([#89](https://github.com/Arskah/diodedj/issues/89)) ([a305ba4](https://github.com/Arskah/diodedj/commit/a305ba44d03d0de21d024bbf31256f148fe2885e))


### Miscellaneous Chores

* **deps:** lock file maintenance ([#61](https://github.com/Arskah/diodedj/issues/61)) ([353fd54](https://github.com/Arskah/diodedj/commit/353fd54ac12181e3027f69b53409c59f9c2332fb))
* **deps:** update commitlint monorepo to v20.5.3 ([#59](https://github.com/Arskah/diodedj/issues/59)) ([61604c2](https://github.com/Arskah/diodedj/commit/61604c22fa12c12139626e5a2618faad46f99edc))
* **deps:** update pnpm/action-setup action to v6.0.4 ([#60](https://github.com/Arskah/diodedj/issues/60)) ([5c5daf1](https://github.com/Arskah/diodedj/commit/5c5daf159f8921b498bfc51d52b20f9b0735bb0d))


### Documentation

* **audio:** document exclusive output investigation, defer step 4 ([e2169df](https://github.com/Arskah/diodedj/commit/e2169dfb56580d1c24bbfe88e9ee4410475b992d))
* **audio:** exclusive output investigation ([#94](https://github.com/Arskah/diodedj/issues/94)) ([e2169df](https://github.com/Arskah/diodedj/commit/e2169dfb56580d1c24bbfe88e9ee4410475b992d))

## [0.7.2](https://github.com/Arskah/diodedj/compare/v0.7.1...v0.7.2) (2026-05-05)


### Bug Fixes

* **ci:** strip CR from jq output in Windows Stage bundles step ([3f4c284](https://github.com/Arskah/diodedj/commit/3f4c284f0bd9b11d0fe76cc3deec4c0e82eccedd))

## [0.7.1](https://github.com/Arskah/diodedj/compare/v0.7.0...v0.7.1) (2026-05-05)


### Bug Fixes

* **build:** add icon.ico + icon.icns for Windows/macOS bundles ([#85](https://github.com/Arskah/diodedj/issues/85)) ([3c71b36](https://github.com/Arskah/diodedj/commit/3c71b3633474109fa80df13362d50d9e58fcdc62))
* **build:** generate icon.ico + icon.icns for Windows/macOS bundles ([3c71b36](https://github.com/Arskah/diodedj/commit/3c71b3633474109fa80df13362d50d9e58fcdc62))


### Miscellaneous Chores

* fix release versioning ([c1d78af](https://github.com/Arskah/diodedj/commit/c1d78af29efb376c3a6b61549c0a88767ed69774))

## [0.7.0](https://github.com/Arskah/diodedj/compare/v0.6.0...v0.7.0) (2026-05-04)


### ⚠ BREAKING CHANGES

* requires a Rust toolchain (stable) and Tauri CLI to build/run; Electron build artifacts and Playwright e2e are no longer produced.

### Features

* port to Tauri 2 with native rodio audio ([#76](https://github.com/Arskah/diodedj/issues/76)) ([caf4118](https://github.com/Arskah/diodedj/commit/caf4118e4987152b90d7464d6db21ea4bbb91ac2))


### Bug Fixes

* **deps:** pin dependencies ([#82](https://github.com/Arskah/diodedj/issues/82)) ([b36f40a](https://github.com/Arskah/diodedj/commit/b36f40a598f2a89d34c3c5f2b28e2873291b8bb6))


### Miscellaneous Chores

* only bump major after v1 ([00d8342](https://github.com/Arskah/diodedj/commit/00d834255bcf8909cc2729b434511215f5c13d20))


### Code Refactoring

* **player:** introduce PlayerBackend abstraction ([#72](https://github.com/Arskah/diodedj/issues/72)) ([d55dd17](https://github.com/Arskah/diodedj/commit/d55dd175e71d76ac1d4aa301720ce5ec860108e8))

## [0.6.0](https://github.com/Arskah/diodedj/compare/v0.5.1...v0.6.0) (2026-05-04)


### Features

* **playlist:** manual jingle/commercial buttons ([#16](https://github.com/Arskah/diodedj/issues/16)) ([#69](https://github.com/Arskah/diodedj/issues/69)) ([dc816a4](https://github.com/Arskah/diodedj/commit/dc816a44eb3df43fbeb31e8199e00703d0eddddb))
* **playlist:** mix jingles and commercials into auto playlist ([#64](https://github.com/Arskah/diodedj/issues/64)) ([ad58e1a](https://github.com/Arskah/diodedj/commit/ad58e1a743352f6a82a8467a2f4aafbd4f086a21)), closes [#17](https://github.com/Arskah/diodedj/issues/17)
* **scanner:** incremental scan via mtime + preserve play_count ([#65](https://github.com/Arskah/diodedj/issues/65)) ([5d1c567](https://github.com/Arskah/diodedj/commit/5d1c567ea9ebabadbe107e5fa3f79be17058b0b4))
* **scanner:** run library scan in background ([#66](https://github.com/Arskah/diodedj/issues/66)) ([89b084f](https://github.com/Arskah/diodedj/commit/89b084f0d860139d95618534ad360d293359cf83))


### Bug Fixes

* **scanner:** prune DB rows for files deleted inside library roots ([#70](https://github.com/Arskah/diodedj/issues/70)) ([8032f83](https://github.com/Arskah/diodedj/commit/8032f8331ffd4952bb2cb25f940a5fd4eb3de318))


### Documentation

* **audio:** design for chromium bypass + cue deck ([a6a6b8a](https://github.com/Arskah/diodedj/commit/a6a6b8a98f90532680b62150453a6455e263f605))


### Tests

* add unit test for logger ([9715df0](https://github.com/Arskah/diodedj/commit/9715df036cb4f51dc43e45111c0f8592c8c0ea2d))
* setup vitest projects ([c4208f3](https://github.com/Arskah/diodedj/commit/c4208f322bd43a7f8124f902a5a7c61dc4c547bd))

## [0.5.1](https://github.com/Arskah/diodedj/compare/v0.5.0...v0.5.1) (2026-04-28)


### Miscellaneous Chores

* consolidate build configs, tighten tsc coverage, fix CI ([#54](https://github.com/Arskah/diodedj/issues/54)) ([67fb41c](https://github.com/Arskah/diodedj/commit/67fb41c0470f98be6ac4830f0436c9adc35f63e2))

## [0.5.0](https://github.com/Arskah/diodedj/compare/v0.4.0...v0.5.0) (2026-04-28)


### Features

* **library:** sortable columns in track search ([#34](https://github.com/Arskah/diodedj/issues/34)) ([193782e](https://github.com/Arskah/diodedj/commit/193782e7170e3444a0dd2197dce4c3e63ab8a14c))
* **library:** sortable columns in track search ([#52](https://github.com/Arskah/diodedj/issues/52)) ([193782e](https://github.com/Arskah/diodedj/commit/193782e7170e3444a0dd2197dce4c3e63ab8a14c))
* **main:** modern native window chrome ([#50](https://github.com/Arskah/diodedj/issues/50)) ([f64bed6](https://github.com/Arskah/diodedj/commit/f64bed6a985799c4d34a71772fae3215cabca308)), closes [#49](https://github.com/Arskah/diodedj/issues/49)
* **playlist:** history-aware prev navigation ([#45](https://github.com/Arskah/diodedj/issues/45)) ([0dbf72f](https://github.com/Arskah/diodedj/commit/0dbf72fec0be688c74c79e66046f8353d6b21aa7))
* **session:** persist playlist and history across restarts ([#53](https://github.com/Arskah/diodedj/issues/53)) ([12e61bd](https://github.com/Arskah/diodedj/commit/12e61bd6862f67f6d8ba9755dc9d6c9a5b7254e6))
* **ui:** reserve traffic light space, move search to library header ([#51](https://github.com/Arskah/diodedj/issues/51)) ([7d49ce6](https://github.com/Arskah/diodedj/commit/7d49ce609888e1a2c0ce8f48955f25bb51841558))


### Bug Fixes

* append history on stop ([94dd403](https://github.com/Arskah/diodedj/commit/94dd4034eed0a97701b0e0bbbfcc9aa93ae0d2be))
* bigger autoplaylist with distinct threshold for generation ([f23892f](https://github.com/Arskah/diodedj/commit/f23892fab5ddd1aeff0eb317240f8fa83fe38dd7))
* **ipc:** correct Handler type to accept varied return types ([34e02f6](https://github.com/Arskah/diodedj/commit/34e02f60bbad4d0d20d309897f51f1b982c09cc4))


### Code Refactoring

* add appendHistory fn ([14f68a5](https://github.com/Arskah/diodedj/commit/14f68a5cba85726da76d63071508e1ee42469c00))
* **playlist:** split current track from queue ([#44](https://github.com/Arskah/diodedj/issues/44)) ([982d5ec](https://github.com/Arskah/diodedj/commit/982d5ec46270fc129abf50a6bd118b95496f77ca))


### Continuous Integration

* add missing permissions for lint job ([#48](https://github.com/Arskah/diodedj/issues/48)) ([a638918](https://github.com/Arskah/diodedj/commit/a6389181dee0e199bbf4bd25f12f68f42e704fad))

## [0.4.0](https://github.com/Arskah/diodedj/compare/v0.3.1...v0.4.0) (2026-04-27)


### Features

* **logging:** adopt electron-log + ffmpeg capture ([2144b5e](https://github.com/Arskah/diodedj/commit/2144b5e4f445669ec58cbdf13d3aca285e56fc2d))
* **media:** seekable transcode + broader format support ([#41](https://github.com/Arskah/diodedj/issues/41)) ([827b50c](https://github.com/Arskah/diodedj/commit/827b50c9c11d85d94346cb88e273cee3436bf9a2))
* **renderer:** show track metadata tooltip on hover ([#42](https://github.com/Arskah/diodedj/issues/42)) ([0db40be](https://github.com/Arskah/diodedj/commit/0db40be31740d8e2c5ce810cc41e13d103fee707))


### Miscellaneous Chores

* migrate prettier config to prettier.config.mjs ([dba4f11](https://github.com/Arskah/diodedj/commit/dba4f11656f52adb7b2891e0f4b386279c19175e))


### Code Refactoring

* **renderer:** convert to Svelte 5 with runes ([#39](https://github.com/Arskah/diodedj/issues/39)) ([9606c2e](https://github.com/Arskah/diodedj/commit/9606c2e8acec0a115b8fb8113f2f061cd175f619))


### Tests

* **main:** cover transcodeToWav across all supported formats ([89e6521](https://github.com/Arskah/diodedj/commit/89e6521a9446397e249c0f45b005bb7d685c5621))

## [0.3.1](https://github.com/Arskah/diodedj/compare/v0.3.0...v0.3.1) (2026-04-26)


### Miscellaneous Chores

* **deps:** update dependency vite to v8 ([#37](https://github.com/Arskah/diodedj/issues/37)) ([5a6cf50](https://github.com/Arskah/diodedj/commit/5a6cf507bfc78f8d492a932aab14e7f8b78586da))
* update pnpm-workspace ([4f3818c](https://github.com/Arskah/diodedj/commit/4f3818c46bee58bb84103735398c7ee844d49402))


### Build System

* migrate to electron-vite ([#35](https://github.com/Arskah/diodedj/issues/35)) ([2f3955d](https://github.com/Arskah/diodedj/commit/2f3955de1256337d23fc43cac1bf2d21e391e82c))

## [0.3.0](https://github.com/Arskah/diodedj/compare/v0.2.1...v0.3.0) (2026-04-26)


### Features

* **main:** persist console output to log file ([b05ee72](https://github.com/Arskah/diodedj/commit/b05ee72d36fcf832005babf71935bdc782424ac3))


### Bug Fixes

* **deps:** pin dependency kysely to 0.28.16 ([#31](https://github.com/Arskah/diodedj/issues/31)) ([278fa19](https://github.com/Arskah/diodedj/commit/278fa19f676b6844e3e05587775c23ceb22f30df))
* **deps:** promote ms to direct dependency ([9c7a6bb](https://github.com/Arskah/diodedj/commit/9c7a6bb7c210f75584d2f06adb519111e2593cb7))
* **scanner:** surface readdir and parse errors ([bcd7ea6](https://github.com/Arskah/diodedj/commit/bcd7ea6889b1dbc452a6a9f9d3834cb18e7449f3))


### Code Refactoring

* **db:** migrate to Kysely with file-based migrations ([cc71d23](https://github.com/Arskah/diodedj/commit/cc71d2320b0ae329261419e56bb93e7b484bdd4a))
* split window from main ([ac7d190](https://github.com/Arskah/diodedj/commit/ac7d19050d012e4749a28fc5a14012ec352b622e))


### Tests

* rm unused unit tests ([497c863](https://github.com/Arskah/diodedj/commit/497c863a1cce5a8413364525e8417a948ba258d9))


### Build System

* **mac:** declare TCC usage descriptions for protected dirs ([d545292](https://github.com/Arskah/diodedj/commit/d545292640ca9b55d4c7f5ad43d4e0adcaf288cf))

## [0.2.1](https://github.com/Arskah/diodedj/compare/v0.2.0...v0.2.1) (2026-04-26)


### Bug Fixes

* **scripts:** use node: prefixed imports in copy-assets ([4614c65](https://github.com/Arskah/diodedj/commit/4614c65407d137a368c694e0e3d9775e7dd7f283))


### Miscellaneous Chores

* **pkg:** cross-platform build script and add repo metadata ([0b43bb9](https://github.com/Arskah/diodedj/commit/0b43bb97010b4dd0dc8dfca44facdc1e4a701afa))

## [0.2.0](https://github.com/Arskah/diodedj/compare/v0.1.0...v0.2.0) (2026-04-26)


### Features

* **playback:** add drag-scrub seek with Range support ([ad6fb81](https://github.com/Arskah/diodedj/commit/ad6fb81ad10fe19e07aabe1b06be06c7172f4ab8))
* **release:** package app with electron-builder and bundle ffmpeg ([ab9070b](https://github.com/Arskah/diodedj/commit/ab9070b7f41bd31052d4d1ff5d36e74fd7dd44d8))
* **ui:** move playback controls to top of app ([ed92269](https://github.com/Arskah/diodedj/commit/ed92269404f65257c4124344f8f2333ecd4d3396))


### Bug Fixes

* **ci:** allowlist electron postinstall for pnpm 10 ([d43ad09](https://github.com/Arskah/diodedj/commit/d43ad09e0173c7bf6a540c388c3bfcc0d6174073))
* **deps:** pin dependencies ([#1](https://github.com/Arskah/diodedj/issues/1)) ([d0dff6b](https://github.com/Arskah/diodedj/commit/d0dff6bf96a613b469f0b89d0a8faaadea21869c))
* **deps:** pin dependencies ([#20](https://github.com/Arskah/diodedj/issues/20)) ([75553b4](https://github.com/Arskah/diodedj/commit/75553b4f5d10884615e6e7b0806c43e55348d82c))
* **playback:** force m4a/mp4 through ffmpeg transcode ([4ff23a1](https://github.com/Arskah/diodedj/commit/4ff23a1b46785feeb3f57ee48d7d95bc0b829a46))


### Miscellaneous Chores

* add commitlint to enforce conventional commits ([0a0e736](https://github.com/Arskah/diodedj/commit/0a0e7368095e2953715ba39dcd2e4e3361303e20))
* **deps:** pin dependencies ([#14](https://github.com/Arskah/diodedj/issues/14)) ([b87699c](https://github.com/Arskah/diodedj/commit/b87699c7a2751a426389b545ec0b76002f56b687))
* **deps:** pin dependency @playwright/test to 1.59.1 ([#21](https://github.com/Arskah/diodedj/issues/21)) ([05c3967](https://github.com/Arskah/diodedj/commit/05c39672acf473fd4987241cc5cb938ff7895d4b))
* **deps:** pin pnpm/action-setup action to v4 ([#15](https://github.com/Arskah/diodedj/issues/15)) ([7bbeee8](https://github.com/Arskah/diodedj/commit/7bbeee8d84e1e1e831bd312d316507707c82a504))
* **deps:** update actions/checkout action to v6 ([#9](https://github.com/Arskah/diodedj/issues/9)) ([e1624e1](https://github.com/Arskah/diodedj/commit/e1624e14cc5efc2193237a4b69a460731bdb4725))
* **deps:** update actions/setup-node action to v6 ([#10](https://github.com/Arskah/diodedj/issues/10)) ([6ceb1ec](https://github.com/Arskah/diodedj/commit/6ceb1ec53befd1c857560e047b765e4ed9386590))
* **deps:** update dependency lint-staged to v16 ([#4](https://github.com/Arskah/diodedj/issues/4)) ([d692358](https://github.com/Arskah/diodedj/commit/d692358b30135b66a72b5fed84b0676305841954))
* **deps:** update dependency vitest to v4 ([#5](https://github.com/Arskah/diodedj/issues/5)) ([87e2150](https://github.com/Arskah/diodedj/commit/87e21501fa3566fc035b177e41502d6861929a4a))
* **deps:** update eslint monorepo to v10 ([#6](https://github.com/Arskah/diodedj/issues/6)) ([cc8230d](https://github.com/Arskah/diodedj/commit/cc8230d0a5388ced11237a76bdef31db3eb14507))
* **deps:** update node.js to v24.15.0 ([#3](https://github.com/Arskah/diodedj/issues/3)) ([6d04966](https://github.com/Arskah/diodedj/commit/6d049669968ae97703259ead54905a0b815d9e9b))
* **deps:** update pnpm to v10 ([#7](https://github.com/Arskah/diodedj/issues/7)) ([72260fa](https://github.com/Arskah/diodedj/commit/72260fac55bbce89b6c05896f692b09cc566de03))
* **deps:** update pnpm/action-setup action to v6 ([#12](https://github.com/Arskah/diodedj/issues/12)) ([78cc55a](https://github.com/Arskah/diodedj/commit/78cc55a50e612dbdfba59011c211777f3dea9123))
* semantic commits for renovate ([50cad24](https://github.com/Arskah/diodedj/commit/50cad242bf6e5db3461cd081c008056f3c635a41))
* **tooling:** consolidate tsconfigs and move tests out of src ([0c25724](https://github.com/Arskah/diodedj/commit/0c25724544dd71a108ebcff9d33f8e9767ba6f74))
* update release-please config ([6adf110](https://github.com/Arskah/diodedj/commit/6adf1107a8b15629b90b0ff2e09d6baa7c4e8b6b))


### Code Refactoring

* **db:** type prepared statements, drop result casts ([53677e3](https://github.com/Arskah/diodedj/commit/53677e3aefe9293c837500bb6b9e1602bf1289b3))
* **main:** extract audio formats to shared module ([73cee06](https://github.com/Arskah/diodedj/commit/73cee0623b96aa0a2127239a17b189819f34242d))
* **main:** extract IPC handlers to ipc module ([37ea5a4](https://github.com/Arskah/diodedj/commit/37ea5a458e5bf4c8fa06802b97dd77bc82973c38))


### Tests

* **e2e:** add Playwright suite covering library, playback, and playlist flows ([afc503e](https://github.com/Arskah/diodedj/commit/afc503e2c2a3089a988383e0a6a09c74281de747))


### Continuous Integration

* add CI workflow and release-please ([37b91d0](https://github.com/Arskah/diodedj/commit/37b91d093c7ec631a44a0d856225cbf98e81094d))
* add release-please manifest config ([cc59b59](https://github.com/Arskah/diodedj/commit/cc59b596ca6bf3616853401a54815035d9080203))
* pin action digests and use custom GitHub App token ([8dba70d](https://github.com/Arskah/diodedj/commit/8dba70d35cb429b97232a955a7666cb8561bc8f6))
* rewrite GH actions ([d434477](https://github.com/Arskah/diodedj/commit/d434477871608f63ab9b68c8aad28f4e1b880fa1))
