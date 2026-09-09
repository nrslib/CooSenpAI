# CooSenpAI bundled motion assets

Source: https://github.com/Undi95/Hanami/tree/6787685c8d40e4e79bffbb0d389b478f32ef88d6/vrma

- idle-7.vrma: derived from Overte idleWS_all.fbx; SHA-256 a4185076435c7797d169a8e2bd8206696805c46d91d8194ecbe589cd6be24d34
- idle-talking-6.vrma: derived from Overte talk_lefthand.fbx; SHA-256 1648d8a03d7e563cad203fa88b80cfd7629e70292cbcc406bc730ac1ffc8bf60

These two media files are licensed under Apache-2.0, independently of Hanami's application source code. The original copyright notice, license text and conversion notice accompany them as Overte-LICENSE.txt, Apache-2.0.txt and Hanami-VRMA-NOTICE.md. The latter describes other upstream assets which are NOT bundled here.

CooSenpAI keeps these binary files unchanged. At runtime it retargets humanoid tracks to the user's model, excludes source facial/look-at tracks, blends idle and reply motion, and limits the reply segment to the first 6.5 seconds. No game assets or avatar models are included.
