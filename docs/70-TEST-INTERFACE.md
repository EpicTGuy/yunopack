# Plan de test de l'interface

L'interface est la seule partie du projet que les tests automatiques ne
couvrent pas : un bouton peut compiler, répondre, et rester illisible ou
introuvable. Ce plan liste ce qu'il faut regarder, et pourquoi.

Il se déroule à la main, dans un navigateur, sur une instance de
développement. Chaque point a un critère vérifiable — pas « c'est joli »,
mais « l'élément est là, à sa place, et se comporte comme ailleurs ».

## Le banc d'essai

    ./tools/apercu.sh     puis  http://127.0.0.1:8080/tools/apercu.html

`tools/apercu.html` charge la vraie `page.html` dans un cadre et remplace
`fetch` et `EventSource` par des relevés pris sur un serveur réel
(`tools/apercu/`). On y passe d'un écran à l'autre — accueil, souhaits,
alternatives, arbitrages, forge muette — et d'une largeur à l'autre sans
serveur, sans réseau, et sans attendre qu'un dépôt s'analyse.

Le bouton **Mesurer** applique les contrôles mécaniques du tableau
ci-dessous à l'écran affiché et liste ce qui cloche : débordement
horizontal, texte suivi mis en grille, élément qui dépasse à droite,
colonne d'action désalignée, cible sous les 24 px de WCAG 2.5.8. Ce qu'il
ne dit pas — lisibilité, contraste, ordre de lecture — reste à l'œil.

Les relevés sont datés : les refaire consiste à rejouer les points d'entrée
du serveur (`/wishlist`, `/alternatives`, `/audit`, un flux `/jobs/:id/events`)
et à réécrire les fichiers correspondants.

## Ce qu'on vérifie partout

| Point | Critère |
|---|---|
| Largeur | Le contenu reste dans 760 px, centré, avec au moins 16 px de marge latérale |
| Débordement | Aucune barre de défilement horizontale, à aucune largeur |
| Alignement | Le bord droit de la colonne d'action s'aligne d'une ligne à l'autre. Les bords gauches peuvent différer : les libellés n'ont pas la même longueur, et les figer gaspillerait la largeur |
| Cible | Tout ce qui se clique fait au moins 24 px de haut (WCAG 2.5.8) |
| Retour | Un bouton qui déclenche une attente le dit pendant l'attente |
| Clavier | `Tab` atteint chaque élément actionnable, `Entrée` l'active |
| Focus | L'élément au clavier porte un contour visible |
| Thème | Clair, sombre et auto donnent un contraste lisible |
| Langue | Aucun texte ne reste en français quand la langue est autre |

## Largeurs à éprouver

- **400 px** — téléphone. Les colonnes s'empilent, rien ne déborde.
- **760 px** — la largeur du contenu. Aucune marge négative.
- **1440 px** — bureau. Le contenu reste centré, il ne s'étire pas.

## Écran par écran

### Empaqueter

1. La page s'ouvre sur cet onglet, les cinq étapes sont visibles.
2. Cliquer sur une étape ouvre son explication ; recliquer la referme.
3. Saisir deux lettres propose des noms ; les flèches les parcourent.
4. Saisir un nom lance une recherche et liste des dépôts.
5. Coller une adresse lance le pipeline directement.
6. Les étapes s'allument dans l'ordre, le minuteur avance.
7. Un dépôt incomplet ouvre le formulaire de choix, avec propositions,
   provenance, pistes cliquables vers les fichiers du dépôt.
8. Un dépôt refusé affiche le motif, et les étapes suivantes restent grises.
9. Un paquet prêt affiche la commande, le téléchargement, la publication.

### Liste de souhaits

1. La liste se charge et s'affiche du plus demandé au moins.
2. Chaque ligne porte une icône, un nombre de voix, un dépôt.
3. Les badges de faisabilité se remplissent sans rester à « … ».
4. Cliquer un badge ouvre le détail ; les motifs et la conduite du projet.
5. Le filtre réduit la liste sans la casser.
6. Une application déjà au catalogue porte son niveau et le bouton d'examen.

### Alternatives

1. Une recherche par nom rend des résultats.
2. Les suggestions apparaissent dès la deuxième lettre.
3. Le bouton AlternativeTo suit le terme saisi.
4. Les applications déjà au catalogue sont signalées, avec leur niveau.
5. L'examen d'un paquet ouvre son bilan et son bouton de correctif.

## Ce qui a déjà été pris en défaut

Ces points ont été cassés au moins une fois ; ils méritent un regard à
chaque passage.

- Les badges restaient à « … » une dizaine de secondes alors que la
  réponse était immédiate.
- Le formulaire de choix réapparaissait après y avoir répondu.
- Le bouton des détails techniques se trouvait en haut de page, sans
  contexte.
- Les icônes ne s'affichaient pas faute d'être relayées par le serveur.
- Une classe `.note` désignait deux choses sans rapport : un paragraphe
  d'explication et une ligne de jauge. La seconde imposait `display: grid`
  à la première, qui se repliait sur cinq lignes étroites au lieu d'une.
- Les onglets portaient le rôle ARIA sans répondre aux flèches.

## Mesures automatisables

Ces vérifications se font depuis la console du navigateur, et valent mieux
qu'un coup d'œil : un défaut de quelques pixels échappe à l'œil et pas à
une mesure.

```js
// Débordement horizontal, à n'importe quelle largeur, via un cadre
const f = document.createElement('iframe');
f.style.cssText = 'width:400px;height:900px;border:0;position:fixed;left:-9999px';
f.src = location.pathname; document.body.append(f);
f.onload = () => { const d = f.contentDocument.documentElement;
  console.log('déborde :', d.scrollWidth > d.clientWidth); };

// Un élément dont le display a été détourné par une collision de classes
[...document.querySelectorAll('p')].filter(e => getComputedStyle(e).display !== 'block')

// Contraste du texte sur le fond, pour chaque thème
// Les bords droits de la colonne d'action
[...new Set([...document.querySelectorAll('.item')]
  .map(i => Math.round(i.lastElementChild.getBoundingClientRect().right)))]
```
