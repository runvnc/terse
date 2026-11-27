#!/usr/bin/env python3
"""story.py: the Low Bell story compiler and its tools (Python 3, standard
library only). STORY.md describes the story format.

    story.py [-o story_data.h] <story dir or files>       compile
    story.py --check <story>                              check only, print sizes
    story.py --draw <room|picture|object> [-o out.svg|out.png] [--flag F]...
             [--hidden] [--beam X,Y] [--no-objects] <story>
    story.py --import-svg drawing.svg [--layer NAME] [--colour C]
             [--scale S] [--offset X,Y]                   print art lines
    story.py --events transcript.txt [-o out.events]       transcript to a
                                                          `terse run` script

A story is every *.story file of a directory, read in name order (or the
files named). Errors name the file and line; the exit status is 1 if there
was an error, 0 otherwise (warnings do not fail).
"""

import argparse
import math
import os
import re
import struct
import sys
import zlib

# --- Fixed numbers shared with lowbell.c (engine.h) ------------------------

SCENE_W, SCENE_H = 640, 256        # art space, screen units
TEXT_COLS = 78                     # wrapped width of the text area
TEXT_ROWS = 12                     # lines of text shown at once
INK_CAP = 230400
INK_ERROR = INK_CAP // 2           # a scene may use at most half the cap
MAX_ROOM_POINTS = 1500
BEAM_RADIUS = 44

DIRS = ['north', 'south', 'east', 'west', 'northeast', 'northwest', 'southeast', 'southwest', 'up', 'down', 'in', 'out']
DIR_WORDS = {
    'north': ['north', 'n'], 'south': ['south', 's'], 'east': ['east', 'e'], 'west': ['west', 'w'],
    'northeast': ['northeast', 'ne'], 'northwest': ['northwest', 'nw'], 'southeast': ['southeast', 'se'],
    'southwest': ['southwest', 'sw'], 'up': ['up', 'u', 'upstairs'], 'down': ['down', 'd', 'downstairs'],
    'in': ['in', 'inside'], 'out': ['out', 'outside'],
}

# Prepositions: id, words.
PREPS = [('in', ['in', 'into', 'inside']), ('on', ['on', 'onto', 'upon']), ('at', ['at', 'towards', 'toward']),
         ('to', ['to']), ('with', ['with', 'using']), ('about', ['about']), ('from', ['from']),
         ('under', ['under', 'beneath', 'below', 'behind']), ('off', ['off'])]
PREP_ID = {name: i + 1 for i, (name, _) in enumerate(PREPS)}

# Action flags (engine.h AF_*).
NOUN, SECOND, META, TOPIC, OPTNOUN, SIGHT = 1, 2, 4, 8, 16, 32

# The built-in actions, in enum order: name, flags, words, default message.
ACTIONS = [
    ('look', META * 0, ['look', 'l'], None),
    ('examine', NOUN | SIGHT, ['examine', 'x', 'inspect', 'check', 'study', 'describe'], 'examine'),
    ('inventory', META, ['inventory', 'i', 'inv'], None),
    ('take', NOUN, ['take', 'get', 'grab', 'pick', 'carry', 'collect', 'hold'], None),
    ('drop', NOUN, ['drop', 'discard', 'leave'], None),
    ('put_in', NOUN | SECOND, ['put', 'insert', 'place', 'stow', 'pour'], None),
    ('put_on', NOUN | SECOND, [], None),
    ('open', NOUN, ['open', 'unfasten'], None),
    ('close', NOUN, ['close', 'shut'], None),
    ('lock', NOUN, ['lock'], None),
    ('unlock', NOUN, ['unlock'], None),
    ('wear', NOUN, ['wear', 'don'], None),
    ('remove', NOUN, ['remove', 'doff'], None),
    ('turn_on', NOUN, ['light', 'ignite', 'kindle', 'switch', 'relight'], None),
    ('turn_off', NOUN, ['extinguish', 'douse', 'snuff', 'quench'], None),
    ('read', NOUN | SIGHT, ['read', 'decipher'], None),
    ('go', 0, ['go', 'walk', 'run', 'head', 'travel', 'proceed', 'step'], None),
    ('wait', 0, ['wait', 'z', 'rest'], 'wait'),
    ('talk', NOUN, ['talk', 'speak', 'greet', 'hail'], None),
    ('ask', NOUN | TOPIC, ['ask', 'question', 'query'], None),
    ('tell', NOUN | TOPIC, ['tell', 'inform'], None),
    ('give', NOUN | SECOND, ['give', 'offer', 'hand', 'pay'], None),
    ('show', NOUN | SECOND, ['show', 'display'], None),
    ('shine', OPTNOUN, ['shine', 'point', 'aim', 'direct', 'beam'], None),
    ('search', NOUN | SIGHT, ['search', 'rummage'], 'search'),
    ('listen', OPTNOUN, ['listen', 'hear'], 'listen'),
    ('smell', OPTNOUN, ['smell', 'sniff'], 'smell'),
    ('touch', NOUN, ['touch', 'feel', 'rub', 'pat', 'stroke'], 'touch'),
    ('push', NOUN, ['push', 'press', 'shove', 'move'], 'push'),
    ('pull', NOUN, ['pull', 'tug', 'yank', 'drag'], 'pull'),
    ('turn', NOUN, ['turn', 'twist', 'rotate', 'wind'], 'turn'),
    ('eat', NOUN, ['eat', 'taste', 'bite'], 'eat'),
    ('drink', NOUN, ['drink', 'sip', 'swallow'], 'drink'),
    ('climb', OPTNOUN, ['climb', 'scale', 'ascend'], 'climb'),
    ('jump', 0, ['jump', 'leap', 'hop'], 'jump'),
    ('sleep', 0, ['sleep', 'nap', 'doze'], 'sleep'),
    ('attack', NOUN, ['attack', 'hit', 'kick', 'break', 'smash', 'punch', 'strike', 'kill'], 'attack'),
    ('blow', NOUN, ['blow'], 'blow'),
    ('yes', META, ['yes', 'y'], 'yes'),
    ('no', META, ['no'], 'no'),
    ('again', META, ['again', 'g'], None),
    ('undo', META, ['undo', 'oops'], None),
    ('save', META, ['save'], None),
    ('restore', META, ['restore', 'load'], None),
    ('restart', META, ['restart'], None),
    ('quit', META, ['quit', 'q'], None),
    ('help', META, ['help', 'about', 'instructions'], None),
    ('hint', META, ['hint', 'hints', 'clue', 'clues'], None),
    ('score', META, ['score', 'achievements', 'points'], None),
]
ACTION_ID = {a[0]: i for i, a in enumerate(ACTIONS)}

# Verb + particle: the action. `trailing`: also when the particle ends the
# sentence after the object ("turn lantern on").
PARTICLES = [
    ('look', 'at', 'examine', 0), ('look', 'in', 'search', 0), ('look', 'inside', 'search', 0),
    ('look', 'into', 'search', 0), ('look', 'under', 'search', 0), ('look', 'behind', 'search', 0),
    ('pick', 'up', 'take', 1), ('put', 'down', 'drop', 1), ('put', 'on', 'wear', 1), ('put', 'out', 'turn_off', 1),
    ('take', 'off', 'remove', 1), ('turn', 'on', 'turn_on', 1), ('switch', 'on', 'turn_on', 1),
    ('turn', 'off', 'turn_off', 1), ('switch', 'off', 'turn_off', 1), ('blow', 'out', 'turn_off', 1),
    ('talk', 'to', 'talk', 0), ('speak', 'to', 'talk', 0), ('speak', 'with', 'talk', 0), ('talk', 'with', 'talk', 0),
    ('listen', 'to', 'listen', 0), ('light', 'up', 'turn_on', 1),
    ('climb', 'up', 'climb', 0), ('climb', 'down', 'climb', 0), ('shine', 'at', 'shine', 0),
    ('point', 'at', 'shine', 0), ('aim', 'at', 'shine', 0), ('get', 'off', 'remove', 1),
]
# Action + preposition: another action (put X on Y; remove X from Y).
PREP_SWITCH = [('put_in', 'on', 'put_on'), ('remove', 'from', 'take'), ('drop', 'in', 'put_in'), ('drop', 'on', 'put_on')]

ARTICLES = ['the', 'a', 'an', 'some', 'my', 'this', 'that', 'these', 'those', 'one']
PRONOUNS = ['it', 'them', 'him', 'her']
ALLS = ['all', 'everything', 'both']
SEPARATORS = ['then', 'and']
IGNORED = ['please', 'carefully', 'quickly', 'gently', 'again_']  # 'again_' never typed

# Word kinds (engine.h WK_*).
WK_VERB, WK_DIR, WK_PREP, WK_NOUN, WK_ADJ, WK_ART, WK_IT, WK_ALL, WK_SEP = 1, 2, 4, 8, 16, 32, 64, 128, 256

# Text placeholders: {name} -> code (engine.h PH_*).
PLACEHOLDERS = ['the', 'The', 'a', 'A', 'the2', 'The2', 'a2', 'word', 'verb', 'list', 'it', 'It', 'time',
                'score', 'max', 'moves', 'room', 'bell', 'choices', 'name', 'is', 'prep', 'Is']
PH_CODE = {p: i + 1 for i, p in enumerate(PLACEHOLDERS)}

# Default responses, one voice; a story overrides any with `message <key>: text`.
MESSAGES = {
    'pardon': 'I beg your pardon?',
    'unknown_word': 'I don\'t know the word "{word}".',
    'no_verb': 'Begin with a verb, as in LOOK, TAKE LANTERN or GO NORTH. HELP lists more.',
    'cant_see': 'You see no {word} here.',
    'which': 'Which {word} do you mean, {choices}?',
    'what1': 'What do you want to {verb}?',
    'what2': 'What do you want to {verb} {the} {prep}?',
    'it_unknown': 'I\'m not sure what "{word}" refers to.',
    'all_bad': 'You can\'t use ALL with that.',
    'nothing_all': 'There is nothing here to take.',
    'one_at_a_time': 'Please name one thing at a time.',
    'too_dark': 'It is too dark to see that.',
    'dark_room': 'It is pitch dark. Anything here is lost in the black.',
    'dark_name': 'Darkness',
    'taken': 'Taken.',
    'already_have': 'You already have {it}.',
    'fixed': '{The} {is} fixed in place.',
    'take_npc': '{The} would not care for that.',
    'carry_full': 'Your hands are full. Put something down first.',
    'dropped': 'Dropped.',
    'not_carried': 'You aren\'t carrying {it}.',
    'take_off_first': 'You\'ll have to take {it} off first.',
    'put_in': 'You put {the} in {the2}.',
    'put_on': 'You put {the} on {the2}.',
    'not_container': 'You can\'t put anything in {the2}.',
    'not_supporter': 'There is no room on {the2} for anything.',
    'container_closed': '{The} {is} closed.',
    'put_self': 'You can\'t put something inside itself.',
    'opened': 'You open {the}.',
    'opened_reveal': 'You open {the}, revealing {list}.',
    'closed': 'You close {the}.',
    'already_open': '{The} {is} already open.',
    'already_closed': '{The} {is} already closed.',
    'not_openable': 'That isn\'t something you can open.',
    'locked': '{The} {is} locked.',
    'unlocked': 'You unlock {the}.',
    'locked_now': 'You lock {the}.',
    'not_lockable': '{The} {is} not something you can lock.',
    'already_locked': '{The} {is} already locked.',
    'already_unlocked': '{The} {is} not locked.',
    'wrong_key': '{The2} doesn\'t fit.',
    'lock_open': 'You\'ll have to close {it} first.',
    'with_key': '(with {the2})',
    'worn': 'You put on {the}.',
    'not_wearable': 'You can\'t wear {it}.',
    'already_worn': 'You\'re already wearing {it}.',
    'removed': 'You take off {the}.',
    'not_worn': 'You aren\'t wearing {it}.',
    'switched_on': 'You light {the}.',
    'switched_off': 'You put out {the}.',
    'already_on': '{The} {is} already lit.',
    'already_off': '{The} {is} not lit.',
    'not_switchable': 'That isn\'t something you can light.',
    'read_nothing': 'There is nothing written on {the}.',
    'no_exit': 'You can\'t go that way.',
    'door_closed': '{The} {is} closed.',
    'go_where': 'Which way? (NORTH, SOUTH, EAST, WEST, UP, DOWN, IN, OUT, or their first letters.)',
    'wait': 'Time passes.',
    'not_npc': 'You can\'t talk to {the}.',
    'talk_default': '{The} nods but says nothing.',
    'ask_unknown': '{The} has nothing to say about that.',
    'give_default': '{The2} doesn\'t seem interested.',
    'show_default': '{The2} glances at {the} without interest.',
    'give_not_npc': 'You can\'t give anything to {the2}.',
    'shine_no_beam': 'You have no lit lantern to shine.',
    'shine': 'You turn the beam on {the}.',
    'shine_where': 'You sweep the beam across the scene.',
    'search': 'You find nothing of interest.',
    'search_in': 'In {the} you find {list}.',
    'search_on': 'On {the} you find {list}.',
    'search_empty': 'There is nothing in {the}.',
    'listen': 'You hear nothing out of the ordinary.',
    'smell': 'You smell nothing unusual.',
    'touch': 'You feel nothing unexpected.',
    'push': 'Nothing happens.',
    'pull': 'Nothing happens.',
    'turn': 'Nothing happens.',
    'eat': 'That isn\'t something you can eat.',
    'drink': 'That isn\'t something you can drink.',
    'climb': 'You can\'t climb that.',
    'jump': 'You jump on the spot. Nothing comes of it.',
    'sleep': 'This is no time for sleeping.',
    'attack': 'Violence won\'t help here.',
    'blow': 'Nothing happens.',
    'yes': 'That was not a question.',
    'no': 'That was not a question.',
    'custom_default': 'Nothing happens.',
    'examine': 'You see nothing special about {the}.',
    'inv_empty': 'You are empty-handed.',
    'inv': 'You are carrying {list}.',
    'worn_suffix': ' (worn)',
    'lit_suffix': ' (lit)',
    'open_suffix': ' (open)',
    'here_list': 'You can see {list} here.',
    'npc_here': '{The} {is} here.',
    'on_list': 'On {the} you see {list}.',
    'in_list': 'In {the} you see {list}.',
    'nothing': 'nothing',
    'and': ' and ',
    'comma': ', ',
    'or': ' or ',
    'arrives': '{The} arrives.',
    'leaves': '{The} leaves.',
    'undone': 'Undone: one move taken back.',
    'undo_none': 'There is nothing more to undo.',
    'saving': 'Saving...',
    'saved': 'Saved as "{word}".',
    'save_failed': 'The game could not be saved.',
    'no_storage': 'This host has no storage for saved games.',
    'restoring': 'Restoring...',
    'restored': 'Restored "{word}".',
    'no_save': 'There is no saved game called "{word}".',
    'bad_save': 'That saved game belongs to another version of this story.',
    'restart_q': 'Start again from the beginning? (Y or N)',
    'quit_q': 'Do you really want to stop playing? (Y or N)',
    'cancelled': 'Then carry on.',
    'again_none': 'There is nothing to repeat.',
    'hint_none': 'There is no hint for where you are now.',
    'hint_more': '(Type HINT again for more.)',
    'hint_last': '(That is the last hint for this.)',
    'score': 'You have scored {score} out of {max}, in {moves} moves.',
    'score_none': 'This story keeps no score. You have made {moves} moves.',
    'achievement': '[{name}]',
    'end_won': '*** You have won ***',
    'end_lost': '*** You have lost ***',
    'end_prompt': 'Type RESTART, RESTORE, UNDO or QUIT.',
    'more': '-- more --',
    'help': 'Type what you want to do, in a few words: LOOK, EXAMINE BOX, TAKE KEY, '
            'PUT KEY IN BOX, ASK WOMAN ABOUT BELL, GO NORTH (or just N). Also: INVENTORY (I), WAIT (Z), '
            'AGAIN (G), UNDO, SAVE, RESTORE, HINT, SCORE, RESTART and QUIT. When the lantern is lit, the arrow '
            'keys and the pointer move its beam over the picture; SHINE LANTERN AT something aims it. '
            'Page Up and Page Down scroll back through the text.',
    'reveal': 'The beam picks out something you had not seen.',
    'need_light': 'You can\'t see well enough to do that.',
    'no_free_text': 'Names of saved games are letters and digits, at most 16.',
}

# Named colours (story may add more).
COLOURS = {
    'white': (240, 240, 240), 'grey': (150, 150, 150), 'gray': (150, 150, 150), 'dim': (80, 80, 90),
    'red': (230, 60, 50), 'green': (70, 220, 90), 'blue': (80, 120, 255), 'cyan': (80, 220, 230),
    'yellow': (250, 230, 90), 'amber': (255, 176, 40), 'orange': (255, 130, 30), 'brown': (170, 110, 60),
    'violet': (180, 110, 255), 'pink': (255, 140, 190), 'moon': (210, 220, 255), 'water': (70, 140, 220),
}

INSTRUMENTS = ['bell', 'chime', 'tick', 'swish', 'horn', 'thud', 'tone']
NOTE_NAMES = {'c': 0, 'd': 2, 'e': 4, 'f': 5, 'g': 7, 'a': 9, 'b': 11}

# Condition and effect opcodes (engine.h C_*, E_*).
C = dict(END=0, FLAG=1, IN=2, CARRYING=3, HERE=4, OBJIN=5, OPEN=6, LOCKED=7, ON=8, WORN=9, VISITED=10,
         DARK=11, HEARS=12, RANDOM=13, BEAM=14, CMP=15)
E = dict(END=0, SAY=1, SET=2, CLEAR=3, MOVE=4, GO=5, STATE=6, EXIT=7, SOUND=8, ADD=9, LET=10, SCHEDULE=11,
         CANCEL=12, END_GAME=13, LOUD=14, CONTINUE=15, LOOK=16, ACHIEVE=17, AIM=18, TOGGLE=19, MINUTES=20, TRY=21)
STATES = ['open', 'closed', 'locked', 'unlocked', 'on', 'off']
CMP_OPS = ['==', '!=', '<', '<=', '>', '>=']
BUILTIN_COUNTERS = ['moves', 'score', 'time', 'bell', 'minute']

# Object kind bits (engine.h K_*).
K = dict(portable=1, container=2, supporter=4, openable=8, lockable=16, wearable=32, light=64, switchable=128,
         readable=256, npc=512, scenery=1024, beam=2048, plural=4096, proper=8192, transparent=16384, door=32768)
# Object state bits (engine.h S_*).
S_OPEN, S_LOCKED, S_ON, S_MOVED = 1, 2, 4, 8

# Event triggers (engine.h T_*).
T = dict(at=1, every=2, when=3, whenever=4, timer=5, noise=6, enter=7, attime=8, everytime=9)

# Layer types and animations (engine.h).
L_NORMAL, L_HIDDEN = 0, 1
A_NONE, A_FLICKER, A_DRIFT, A_BLINK = 0, 1, 2, 3


class StoryError(Exception):
    pass


class Diag:
    def __init__(self):
        self.errors = []
        self.warnings = []

    def error(self, where, msg):
        self.errors.append(f'{where}: error: {msg}')

    def warn(self, where, msg):
        self.warnings.append(f'{where}: warning: {msg}')


D = Diag()


def closest(name, names):
    best, bd = None, 99
    for n in names:
        d = edit_distance(name, n)
        if d < bd:
            best, bd = n, d
    if best is not None and bd <= max(2, len(name) // 3):
        return f' (did you mean "{best}"?)'
    return ''


def edit_distance(a, b):
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = cur
    return prev[-1]


# --- Reading: lines and the indentation tree --------------------------------


class Line:
    def __init__(self, file, no, raw):
        self.file, self.no, self.raw = file, no, raw.rstrip('\n').rstrip('\r')
        expanded = self.raw.replace('\t', '    ')
        self.indent = len(expanded) - len(expanded.lstrip(' '))
        self.body = expanded.strip()
        self.blank = self.body == ''
        self.comment = self.body.startswith('#')
        self.text = strip_comment(self.body)

    def where(self):
        return f'{self.file}:{self.no}'


def strip_comment(s):
    out, q = [], None
    for i, ch in enumerate(s):
        if q:
            if ch == q or (q == '“' and ch == '”'):
                q = None
        elif ch in '"“':
            q = ch
        elif ch == '#' and (i == 0 or s[i - 1] in ' \t'):
            break
        out.append(ch)
    return ''.join(out).rstrip()


class Node:
    def __init__(self, line, idx):
        self.line, self.idx, self.children, self.end = line, idx, [], idx

    @property
    def text(self):
        return self.line.text

    def where(self):
        return self.line.where()


def read_lines(paths):
    lines = []
    for p in paths:
        with open(p, encoding='utf-8') as f:
            for i, raw in enumerate(f, 1):
                try:
                    name = os.path.relpath(p)
                except ValueError:  # on another drive (Windows)
                    name = '..'
                lines.append(Line(p if name.startswith("..") else name, i, raw))
    return lines


def build_tree(lines):
    """Each significant line is a node; the lines indented under it are its
    children. node.end is the last physical line of its block (blank and
    comment lines included), for text blocks."""
    root = Node(Line('-', 0, ''), -1)
    root.line.indent = -1
    stack = [root]
    for i, ln in enumerate(lines):
        if ln.blank or ln.comment:
            continue
        while stack[-1].line.indent >= ln.indent:
            stack.pop()
        n = Node(ln, i)
        stack[-1].children.append(n)
        stack.append(n)
    # Ends: the line before the next significant line at the same or lower indent.
    sig = [i for i, l in enumerate(lines) if not (l.blank or l.comment)]

    def set_end(n, limit):
        kids = n.children
        for k, c in enumerate(kids):
            nxt = kids[k + 1].idx - 1 if k + 1 < len(kids) else limit
            set_end(c, nxt)
        # Trim trailing blank lines of a block.
        e = limit
        while e > n.idx and (lines[e].blank or lines[e].comment):
            e -= 1
        n.end = e
    set_end(root, len(lines) - 1)
    del sig
    return root


def tokens(s, where):
    """Splits a statement into words and quoted strings (tuples ('"', text))."""
    out, i = [], 0
    while i < len(s):
        ch = s[i]
        if ch in ' \t':
            i += 1
        elif ch == ',':
            out.append(',')
            i += 1
        elif ch in '"“':
            close = '"' if ch == '"' else '”'
            j = s.find(close, i + 1)
            if j < 0:
                D.error(where, 'a quoted text has no closing quote')
                j = len(s)
            out.append(('"', s[i + 1:j]))
            i = j + 1
        else:
            j = i
            while j < len(s) and s[j] not in ' \t,"“':
                j += 1
            out.append(s[i:j])
            i = j
    return out


def block_text(lines, node, first=''):
    """The text of a block: `first` (after the colon), then the lines
    indented under the node. Lines are joined into paragraphs; a blank line
    ends a paragraph; a line starting with | keeps its own line."""
    raw = []
    if first.strip():
        raw.append(first.strip())
    for i in range(node.idx + 1, node.end + 1):
        ln = lines[i]
        if ln.comment:
            continue
        raw.append('' if ln.blank else ln.body)
    return join_text(raw)


def join_text(raw):
    paras, cur = [], []
    out = []
    for r in raw:
        if r == '':
            if cur:
                paras.append(cur)
                cur = []
            continue
        cur.append(r)
    if cur:
        paras.append(cur)
    for p in paras:
        s = ''
        for r in p:
            if r.startswith('|'):
                s += ('\n' if s else '') + r[1:].lstrip(' ') if not s.endswith('\n') else r[1:].lstrip(' ')
                s += '\n'
            else:
                if s and not s.endswith('\n'):
                    s += ' '
                s += r
        out.append(s.rstrip('\n'))
    return '\n\n'.join(out)


def key_and_rest(text):
    """'say: hello' -> ('say', 'hello', True); 'exit north x' -> ('exit', 'north x', False)."""
    m = re.match(r'^([A-Za-z_][\w.\-]*)(\s*:\s*|\s+|$)(.*)$', text)
    if not m:
        return text, '', False
    colon = ':' in m.group(2)
    return m.group(1), m.group(3), colon


# --- The model ---------------------------------------------------------------


class Thing:
    def __init__(self, ident, where):
        self.id, self.where = ident, where


class Room(Thing):
    def __init__(self, ident, where):
        super().__init__(ident, where)
        self.name = ident.replace('_', ' ').title()
        self.desc = ''
        self.dark = False
        self.notes = []          # (cond tokens, where, text)
        self.exits = []          # dict
        self.art = None
        self.spots = []
        self.ambient = None      # (sound, every, where)
        self.index = 0


class Obj(Thing):
    def __init__(self, ident, where, npc=False):
        super().__init__(ident, where)
        self.name = ident.replace('_', ' ')
        self.article = None
        self.nouns, self.adjs = [], []
        self.loc = None          # (token, where)
        self.also = None
        self.kinds = set(['npc'] if npc else [])
        self.portable = not npc
        self.state = 0
        self.key = None
        self.desc = ''
        self.initial = ''
        self.readable = ''
        self.here = ''
        self.talk = ''
        self.unknown = ''
        self.arrives = ''
        self.leaves = ''
        self.topics = []         # (words, text, where)
        self.idle = (0, [])
        self.route = []          # (move, loc token, where)
        self.at = None
        self.top = None
        self.figure = None
        self.index = 0
        self.npc = npc


class Layer:
    def __init__(self, name, where):
        self.name, self.where = name, where
        self.colour = COLOURS['white']
        self.cond = None
        self.hidden = False
        self.faint = False
        self.reveal = None
        self.reveal_text = ''
        self.anchor = None
        self.anim = (A_NONE, 0, 0, 0, 0)
        self.shapes = []         # list of list of (x, y) in units (floats)


class Art:
    def __init__(self, where, relative=False):
        self.where, self.relative, self.layers = where, relative, []


class Rule:
    def __init__(self, kind, where):
        self.kind, self.where = kind, where   # 'on' or 'after'
        self.action = None
        self.obj1 = self.obj2 = None
        self.dir = None
        self.conds = []          # list of (tokens, where)
        self.effects = []        # list of (node)


class Event:
    def __init__(self, ident, where):
        self.id, self.where = ident, where
        self.trigger = None
        self.a = self.b = 0
        self.cond = []
        self.effects = []
        self.index = 0


class Story:
    def __init__(self):
        self.title = 'Untitled'
        self.author = ''
        self.intro = ''
        self.start = None
        self.carry = 0
        self.clock = None        # (start minutes, step, 24h)
        self.beam = BEAM_RADIUS
        self.status = '{room}||{moves} moves'
        self.max_score = 0
        self.help = ''
        self.step = None
        self.minutes = {}
        self.hidden_count = 0
        self.default_minutes = 1
        self.rooms, self.objs, self.events, self.hints, self.sounds, self.pictures = {}, {}, {}, [], {}, {}
        self.rules = []
        self.verbs = []          # (action, words, default text, flags, where)
        self.custom_actions = []  # names
        self.custom_defaults = {}
        self.flags = {}          # name -> (index, initially set, where)
        self.flag_uses = {}      # name -> [set count, test count]
        self.counters = {}       # name -> (index, initial)
        self.messages = dict(MESSAGES)
        self.achievements = {}   # id -> (index, title, points)
        self.colours = dict(COLOURS)
        self.room_order, self.obj_order, self.event_order = [], [], []


ST = Story()


# --- Parsing ---------------------------------------------------------------


def parse(lines):
    root = build_tree(lines)
    for n in root.children:
        if n.line.indent != 0:
            D.error(n.where(), 'a declaration must start at the left margin')
        key, rest, colon = key_and_rest(n.text)
        f = TOP.get(key)
        if not f:
            D.error(n.where(), f'unknown declaration "{key}"' + closest(key, TOP))
            continue
        f(lines, n, rest, colon)


def need_ident(t, where, what):
    if not isinstance(t, str) or not re.match(r'^[a-z][a-z0-9_]*$', t):
        D.error(where, f'{what} must be a name of lower-case letters, digits and _ (got {t!r})')
        return None
    return t


def p_game(lines, n, rest, colon):
    for c in n.children:
        key, r, colon = key_and_rest(c.text)
        w = c.where()
        toks = tokens(r, w)
        if key == 'title':
            ST.title = unquote(r)
        elif key == 'author':
            ST.author = unquote(r)
        elif key == 'start':
            ST.start = (r.strip(), w)
        elif key == 'carry':
            ST.carry = int_or(r, w, 0, 50)
        elif key == 'beam':
            ST.beam = int_or(r, w, 8, 120)
        elif key == 'score':
            ST.max_score = int_or(r, w, 0, 30000)
        elif key == 'clock':
            parts = r.split()
            if not parts:
                D.error(w, 'clock <start time> [24h]: clock 21:00')
                continue
            st = parse_time(parts[0], w)
            ST.clock = (st, 1, '24h' in parts[1:])
            if len(parts) > 1 and parts[1].isdigit():
                ST.default_minutes = int_or(parts[1], w, 0, 600)
        elif key == 'minutes':
            ST.default_minutes = int_or(r, w, 0, 600)
        elif key == 'status':
            ST.status = block_text(lines, c, r) if colon else unquote(r)
        elif key == 'intro':
            ST.intro = block_text(lines, c, r)
        elif key == 'help':
            ST.help = block_text(lines, c, r)
        elif key == 'step':
            ST.step = (r.strip(), w)
        else:
            D.error(w, f'unknown game setting "{key}"' + closest(key, ['title', 'author', 'start', 'carry', 'beam', 'score', 'clock', 'status', 'intro', 'help', 'step', 'minutes']))
        del toks


def unquote(s):
    s = s.strip()
    if len(s) >= 2 and s[0] in '"“' and s[-1] in '"”':
        return s[1:-1]
    return s


def int_or(s, where, lo, hi):
    try:
        v = int(str(s).strip())
    except ValueError:
        D.error(where, f'expected a whole number, got {s!r}')
        return lo
    if not lo <= v <= hi:
        D.error(where, f'{v} is out of range ({lo} to {hi})')
    return v


def parse_time(s, where):
    m = re.match(r'^(\d{1,2}):(\d\d)$', s)
    if not m:
        D.error(where, f'expected a time such as 21:00, got {s!r}')
        return 0
    return int(m.group(1)) * 60 + int(m.group(2))


def p_colour(lines, n, rest, colon):
    t = rest.split()
    if len(t) != 4:
        D.error(n.where(), 'colour <name> <red> <green> <blue> (0 to 255 each)')
        return
    ST.colours[t[0]] = tuple(int_or(v, n.where(), 0, 255) for v in t[1:])


def p_flag(lines, n, rest, colon):
    t = rest.split()
    if not t:
        D.error(n.where(), 'flag <name> [set]')
        return
    name = need_ident(t[0], n.where(), 'a flag')
    if name:
        declare_flag(name, n.where(), len(t) > 1 and t[1] == 'set')


def declare_flag(name, where, initially=False):
    if name not in ST.flags:
        ST.flags[name] = [len(ST.flags), initially, where]
        ST.flag_uses[name] = [0, 0]
    elif initially:
        ST.flags[name][1] = True
    return ST.flags[name][0]


def p_counter(lines, n, rest, colon):
    t = rest.replace('=', ' ').split()
    if not t:
        D.error(n.where(), 'counter <name> [initial value]')
        return
    name = need_ident(t[0], n.where(), 'a counter')
    if name in BUILTIN_COUNTERS:
        D.error(n.where(), f'"{name}" is a built-in counter')
        return
    if name:
        ST.counters[name] = (len(ST.counters) + len(BUILTIN_COUNTERS), int_or(t[1], n.where(), -32768, 32767) if len(t) > 1 else 0)


def p_message(lines, n, rest, colon):
    key, r, colon = key_and_rest(rest)
    if key not in MESSAGES:
        D.error(n.where(), f'unknown message "{key}"' + closest(key, MESSAGES))
        return
    ST.messages[key] = block_text(lines, n, r) if colon else unquote(r)


def p_room(lines, n, rest, colon):
    t = tokens(rest, n.where())
    if not t:
        D.error(n.where(), 'room <name> ["Title"]')
        return
    ident = need_ident(t[0], n.where(), 'a room')
    if not ident:
        return
    if ident in ST.rooms or ident in ST.objs:
        D.error(n.where(), f'"{ident}" is already declared')
        return
    r = Room(ident, n.where())
    if len(t) > 1 and isinstance(t[1], tuple):
        r.name = t[1][1]
    ST.rooms[ident] = r
    ST.room_order.append(r)
    for c in n.children:
        key, rest2, colon2 = key_and_rest(c.text)
        w = c.where()
        if key == 'description':
            r.desc = block_text(lines, c, rest2)
        elif key == 'dark':
            r.dark = True
        elif key == 'note':
            # note if <cond>: text
            m = re.match(r'^note\s+if\s+(.*?)\s*:\s*(.*)$', c.text)
            if not m:
                D.error(w, 'note if <condition>: text')
                continue
            r.notes.append((tokens(m.group(1), w), w, block_text(lines, c, m.group(2))))
        elif key == 'exit':
            p_exit(r, lines, c, rest2)
        elif key == 'art':
            r.art = p_art(lines, c, relative=False)
        elif key == 'spot':
            xy = numbers(rest2, w)
            if len(xy) != 2:
                D.error(w, 'spot <x> <y>')
            else:
                r.spots.append((xy[0], xy[1], w))
        elif key == 'ambient':
            t2 = rest2.split()
            if len(t2) != 3 or t2[1] != 'every':
                D.error(w, 'ambient <sound> every <frames>')
            else:
                r.ambient = (t2[0], int_or(t2[2], w, 30, 6000), w)
        else:
            D.error(w, f'unknown room setting "{key}"' + closest(key, ['description', 'dark', 'note', 'exit', 'art', 'spot', 'ambient']))


def p_exit(r, lines, c, rest):
    w = c.where()
    t = tokens(rest, w)
    if not t or t[0] not in DIRS and t[0] not in sum(DIR_WORDS.values(), []):
        D.error(w, 'exit <direction> <room> [through <door>] [if <condition> [otherwise "text"]], or exit <direction> "text"')
        return
    d = t[0]
    for k, v in DIR_WORDS.items():
        if d in v:
            d = k
    e = {'dir': d, 'to': None, 'door': None, 'cond': None, 'refuse': '', 'where': w}
    i = 1
    if i < len(t) and isinstance(t[i], tuple):
        e['refuse'] = t[i][1]
        i += 1
    elif i < len(t):
        e['to'] = t[i]
        i += 1
    if i < len(t) and t[i] == 'through':
        if i + 1 >= len(t):
            D.error(w, 'through <door object>')
            return
        e['door'] = t[i + 1]
        i += 2
    if i < len(t) and t[i] == 'if':
        j = i + 1
        k = j
        while k < len(t) and t[k] != 'otherwise':
            k += 1
        e['cond'] = t[j:k]
        if k < len(t):
            if k + 1 < len(t) and isinstance(t[k + 1], tuple):
                e['refuse'] = t[k + 1][1]
            else:
                D.error(w, 'otherwise "text"')
        i = len(t)
    if i < len(t):
        D.error(w, f'unexpected {t[i]!r} in exit')
    if any(x['dir'] == d for x in r.exits):
        D.error(w, f'{r.id} already has an exit {d}')
    r.exits.append(e)


def numbers(s, where):
    out = []
    for p in s.replace(',', ' ').split():
        try:
            out.append(float(p))
        except ValueError:
            D.error(where, f'expected a number, got {p!r}')
    return out


def p_art(lines, n, relative):
    art = Art(n.where(), relative)
    for c in n.children:
        key, rest, colon = key_and_rest(c.text)
        if key != 'layer':
            # A figure may give its shapes without a layer.
            if relative and key in SHAPES:
                if not art.layers:
                    art.layers.append(Layer('figure', c.where()))
                shape(art.layers[-1], c)
                continue
            if relative and key in ('colour', 'color'):
                if not art.layers:
                    art.layers.append(Layer('figure', c.where()))
                art.layers[-1].colour = colour(rest.strip(), c.where())
                continue
            D.error(c.where(), 'art holds layers: layer <name> [options], then shapes indented under it')
            continue
        art.layers.append(p_layer(c, rest))
    return art


def colour(name, where):
    if name in ST.colours:
        return ST.colours[name]
    m = re.match(r'^#?([0-9a-fA-F]{6})$', name)
    if m:
        v = int(m.group(1), 16)
        return (v >> 16, (v >> 8) & 255, v & 255)
    D.error(where, f'unknown colour "{name}"' + closest(name, ST.colours))
    return COLOURS['white']


def p_layer(c, rest):
    w = c.where()
    t = tokens(rest, w)
    if not t:
        D.error(w, 'layer <name> [colour C] [hidden] [faint] [reveal FLAG ["text"]] [at X Y] [flicker | drift DX DY FRAMES | blink ON OFF] [if CONDITION]')
        return Layer('?', w)
    L = Layer(t[0], w)
    i = 1
    while i < len(t):
        k = t[i]
        if k in ('colour', 'color') and i + 1 < len(t):
            L.colour = colour(t[i + 1], w)
            i += 2
        elif k == 'hidden':
            L.hidden = True
            i += 1
        elif k == 'faint':
            L.faint = True
            i += 1
        elif k == 'reveal' and i + 1 < len(t):
            L.reveal = (t[i + 1], w)
            L.hidden = True
            i += 2
            if i < len(t) and isinstance(t[i], tuple):
                L.reveal_text = t[i][1]
                i += 1
        elif k == 'at' and i + 2 < len(t):
            L.anchor = (num(t[i + 1], w), num(t[i + 2], w))
            i += 3
        elif k == 'flicker':
            L.anim = (A_FLICKER, 0, 0, 0, 0)
            i += 1
        elif k == 'drift' and i + 3 < len(t):
            L.anim = (A_DRIFT, int(num(t[i + 1], w)), int(num(t[i + 2], w)), int(num(t[i + 3], w)), 0)
            if not (-100 <= L.anim[1] <= 100 and -100 <= L.anim[2] <= 100 and 8 <= L.anim[3] <= 2000):
                D.error(w, 'drift <dx> <dy> <frames>: dx and dy at most 100 units, the period 8 to 2000 frames')
            i += 4
        elif k == 'blink' and i + 2 < len(t):
            L.anim = (A_BLINK, 0, 0, int(num(t[i + 1], w)), int(num(t[i + 2], w)))
            if not (1 <= L.anim[3] <= 255 and 1 <= L.anim[4] <= 255):
                D.error(w, 'blink <frames on> <frames off>: 1 to 255 each')
            i += 3
        elif k == 'if':
            L.cond = (t[i + 1:], w)
            break
        else:
            D.error(w, f'unexpected {k!r} in layer' + (closest(k, ['colour', 'hidden', 'faint', 'reveal', 'at', 'flicker', 'drift', 'blink', 'if']) if isinstance(k, str) else ''))
            i += 1
    for s in c.children:
        shape(L, s)
    return L


def num(t, where):
    try:
        return float(t)
    except (ValueError, TypeError):
        D.error(where, f'expected a number, got {t!r}')
        return 0.0


SHAPES = ['line', 'poly', 'rect', 'circle', 'ellipse', 'arc', 'dots']


def shape(L, s):
    key, rest, colon = key_and_rest(s.text)
    w = s.where()
    v = numbers(rest, w) if key != 'arc' else []
    pts = list(zip(v[0::2], v[1::2]))
    if key == 'line':
        if len(v) < 4 or len(v) % 2:
            D.error(w, 'line x1 y1 x2 y2 [x3 y3 ...]: pairs of numbers, at least two points')
            return
        L.shapes.append((pts, w))
    elif key == 'poly':
        if len(v) < 6 or len(v) % 2:
            D.error(w, 'poly x1 y1 x2 y2 x3 y3 [...]: at least three points; the last joins the first')
            return
        L.shapes.append((pts + [pts[0]], w))
    elif key == 'rect':
        if len(v) != 4:
            D.error(w, 'rect x y width height')
            return
        x, y, ww, hh = v
        L.shapes.append(([(x, y), (x + ww, y), (x + ww, y + hh), (x, y + hh), (x, y)], w))
    elif key in ('circle', 'ellipse', 'arc'):
        if key == 'circle' and len(v) == 3:
            cx, cy, rx = v
            ry, a0, a1 = rx, 0, 360
        elif key == 'ellipse' and len(v) == 4:
            cx, cy, rx, ry = v
            a0, a1 = 0, 360
        elif key == 'arc':
            m = re.match(r'^\s*(\S+)[\s,]+(\S+)[\s,]+(\S+)(?:[\s,]+(\S+))?\s+from\s+(\S+)\s+to\s+(\S+)\s*$', rest)
            if not m:
                D.error(w, 'arc cx cy r [ry] from <degrees> to <degrees> (0 is east, 90 north)')
                return
            cx, cy, rx = num(m.group(1), w), num(m.group(2), w), num(m.group(3), w)
            ry = num(m.group(4), w) if m.group(4) else rx
            a0, a1 = num(m.group(5), w), num(m.group(6), w)
            if a1 <= a0:
                a1 += 360
        else:
            D.error(w, {'circle': 'circle cx cy r', 'ellipse': 'ellipse cx cy rx ry'}[key])
            return
        span = a1 - a0
        n = max(3, min(48, int(math.ceil(span / 360 * max(10, min(40, (rx + ry) / 2.2))))))
        p = [(cx + rx * math.cos(math.radians(a0 + span * k / n)), cy - ry * math.sin(math.radians(a0 + span * k / n))) for k in range(n + 1)]
        L.shapes.append((p, w))
    elif key == 'dots':
        if len(v) < 2 or len(v) % 2:
            D.error(w, 'dots x y [x y ...]')
            return
        for p in pts:
            L.shapes.append(([p], w))
    else:
        D.error(w, f'unknown shape "{key}"' + closest(key, SHAPES))


def p_object(lines, n, rest, colon, npc=False):
    t = tokens(rest, n.where())
    if not t:
        D.error(n.where(), ('npc' if npc else 'object') + ' <name> ["printed name"]')
        return
    ident = need_ident(t[0], n.where(), 'an object')
    if not ident:
        return
    if ident in ST.objs or ident in ST.rooms:
        D.error(n.where(), f'"{ident}" is already declared')
        return
    o = Obj(ident, n.where(), npc)
    if len(t) > 1 and isinstance(t[1], tuple):
        o.name = t[1][1]
    ST.objs[ident] = o
    ST.obj_order.append(o)
    for c in n.children:
        key, r, colon2 = key_and_rest(c.text)
        w = c.where()
        words = [x for x in tokens(r, w) if x != ',']
        if key in ('nouns', 'noun'):
            o.nouns += [x.lower() for x in words if isinstance(x, str)]
        elif key in ('adjectives', 'adjective', 'adj'):
            o.adjs += [x.lower() for x in words if isinstance(x, str)]
        elif key == 'article':
            if r.strip() not in ('a', 'an', 'some', 'the', 'none'):
                D.error(w, 'article a, an, some, the or none')
            o.article = r.strip()
        elif key == 'in':
            o.loc = (r.strip(), w)
        elif key == 'also':
            m = re.match(r'^in\s+(\S+)$', r.strip())
            if not m:
                D.error(w, 'also in <room>')
            else:
                o.also = (m.group(1), w)
        elif key in ('readable', 'text') and (colon2 or r):
            o.kinds.add('readable')
            o.readable = block_text(lines, c, r)
        elif key in K:
            o.kinds.add(key)
            if key == 'door':
                o.kinds |= {'scenery', 'openable'}
            if key == 'npc':
                o.npc = True
        elif key == 'fixed':
            o.portable = False
        elif key == 'scenery':
            o.kinds.add('scenery')
        elif key == 'open':
            o.state |= S_OPEN
        elif key == 'closed':
            o.state &= ~S_OPEN
        elif key == 'locked':
            o.state |= S_LOCKED
            o.kinds.add('lockable')
        elif key == 'lit' or key == 'on':
            o.state |= S_ON
        elif key == 'key':
            o.key = (r.strip(), w)
            o.kinds.add('lockable')
        elif key == 'description':
            o.desc = block_text(lines, c, r)
        elif key == 'initial':
            o.initial = block_text(lines, c, r)
        elif key in ('readable', 'text'):
            o.kinds.add('readable')
            o.readable = block_text(lines, c, r)
        elif key == 'here':
            o.here = block_text(lines, c, r)
        elif key == 'talk':
            o.talk = block_text(lines, c, r)
        elif key == 'unknown':
            o.unknown = block_text(lines, c, r)
        elif key == 'arrives':
            o.arrives = block_text(lines, c, r)
        elif key == 'leaves':
            o.leaves = block_text(lines, c, r)
        elif key == 'topic':
            m = re.match(r'^topic\s+(.*?)\s*:\s*(.*)$', c.text)
            if not m:
                D.error(w, 'topic <word>[, <word>...]: text')
                continue
            ws = [x.strip().lower() for x in m.group(1).split(',') if x.strip()]
            o.topics.append((ws, block_text(lines, c, m.group(2)), w))
        elif key == 'idle':
            m = re.match(r'^idle\s+every\s+(\d+)\s*:?\s*$', c.text)
            if not m:
                D.error(w, 'idle every <moves>: then one line of text a line')
                continue
            texts = [lines[i].body for i in range(c.idx + 1, c.end + 1) if not lines[i].blank and not lines[i].comment]
            o.idle = (int(m.group(1)), texts)
        elif key == 'route':
            for rc in c.children:
                rt = rc.text.split()
                if len(rt) != 2:
                    D.error(rc.where(), 'route lines: <move or time> <room or nowhere>')
                    continue
                o.route.append((rt[0], rt[1], rc.where()))
        elif key == 'at':
            xy = numbers(r, w)
            if len(xy) != 2:
                D.error(w, 'at <x> <y>')
            else:
                o.at = (xy[0], xy[1])
        elif key == 'top':
            xy = numbers(r, w)
            if len(xy) != 2:
                D.error(w, 'top <dx> <dy>: where things on it are drawn, from its position')
            else:
                o.top = (xy[0], xy[1])
        elif key == 'figure':
            o.figure = p_art(lines, c, relative=True)
        else:
            D.error(w, f'unknown object setting "{key}"' + closest(key, OBJ_KEYS))
    if 'scenery' in o.kinds or 'door' in o.kinds or o.npc:
        o.portable = False
    if 'wearable' in o.kinds:
        o.portable = True


OBJ_KEYS = ['nouns', 'adjectives', 'article', 'in', 'also', 'fixed', 'scenery', 'open', 'closed', 'locked', 'lit', 'key',
            'description', 'initial', 'readable', 'here', 'talk', 'unknown', 'topic', 'idle', 'route', 'at', 'top',
            'figure', 'arrives', 'leaves'] + list(K)


def p_npc(lines, n, rest, colon):
    p_object(lines, n, rest, colon, npc=True)


def p_verb(lines, n, rest, colon):
    # verb <action> [= | +=] word, "two words", ...   [then children: default: text]
    m = re.match(r'^([a-z_][a-z0-9_]*)\s*(\+?=|:)?\s*(.*)$', rest.strip())
    if not m:
        D.error(n.where(), 'verb <action>: word, word, "two words"')
        return
    action = m.group(1)
    words = []
    for x in tokens(m.group(3), n.where()):
        if x == ',':
            continue
        words.append((x[1] if isinstance(x, tuple) else x).lower())
    default, flags = None, NOUN
    for c in n.children:
        key, r, colon2 = key_and_rest(c.text)
        if key == 'default':
            default = block_text(lines, c, r)
        elif key == 'alone':
            flags = OPTNOUN
        elif key == 'minutes':
            ST.minutes[action] = (int_or(r, c.where(), 0, 600), c.where())
        else:
            D.error(c.where(), 'verb settings: default: text, alone, minutes N')
    ST.verbs.append((action, words, default, flags, n.where()))
    if action not in ACTION_ID and action not in ST.custom_actions:
        ST.custom_actions.append(action)


def p_rule(kind):
    def f(lines, n, rest, colon):
        w = n.where()
        r = Rule(kind, w)
        head = rest
        if ':' in head and colon is False:
            pass
        # Inline condition: on take oar if in jetty
        m = re.match(r'^(.*?)\s+if\s+(.*)$', head)
        inline = None
        if m:
            head, inline = m.group(1), m.group(2)
        t = [x for x in tokens(head, w) if x != ',']
        if not t:
            D.error(w, f'{kind} <verb> [<object> [<preposition> <object>]]')
            return
        r.head = t
        if inline:
            r.conds.append((tokens(inline, w), w))
        for c in n.children:
            key, rr, colon2 = key_and_rest(c.text)
            if key == 'if':
                r.conds.append((tokens(rr, c.where()), c.where()))
            else:
                r.effects.append(c)
        r.lines = lines
        ST.rules.append(r)
    return f


def p_minutes(lines, n, rest, colon):
    t = rest.split()
    if len(t) != 2:
        D.error(n.where(), 'minutes <action or verb> <minutes it takes>')
        return
    ST.minutes[t[0]] = (int_or(t[1], n.where(), 0, 600), n.where())


def p_event(lines, n, rest, colon):
    w = n.where()
    t = rest.split(None, 2)
    if len(t) < 2:
        D.error(w, 'event <name> at <move|time> | every <n> [from <m>] | when <cond> | whenever <cond> | timer | noise | enter <room>')
        return
    ident = need_ident(t[0], w, 'an event')
    if not ident:
        return
    if ident in ST.events:
        D.error(w, f'event "{ident}" is already declared')
        return
    ev = Event(ident, w)
    kind = t[1]
    more = t[2] if len(t) > 2 else ''
    ev.trigger = kind
    if kind not in T:
        D.error(w, f'unknown trigger "{kind}"' + closest(kind, T))
    ev.raw = more
    ev.lines = lines
    for c in n.children:
        key, rr, colon2 = key_and_rest(c.text)
        if key == 'if':
            ev.cond.append((tokens(rr, c.where()), c.where()))
        elif key == 'warning':
            ev.warning = (block_text(lines, c, rr), c.where())
        else:
            ev.effects.append(c)
    m = re.match(r'^(.*?)\s+warn\s+([\d\s]+)$', more)
    if m:
        ev.raw = m.group(1)
        ev.warn = [int(x) for x in m.group(2).split()]
    ST.events[ident] = ev
    ST.event_order.append(ev)


def p_hint(lines, n, rest, colon):
    w = n.where()
    h = {'id': rest.strip(), 'where': w, 'cond': [], 'texts': []}
    for c in n.children:
        key, rr, colon2 = key_and_rest(c.text)
        if key == 'while':
            h['cond'].append((tokens(rr, c.where()), c.where()))
        elif c.text.startswith('-'):
            raw = [c.text[1:].strip()] + [lines[i].body for i in range(c.idx + 1, c.end + 1) if not lines[i].blank and not lines[i].comment]
            h['texts'].append((join_text(raw), c.where()))
        else:
            D.error(c.where(), 'a hint holds `while <condition>` and hints, each a line starting with -')
    if not h['texts']:
        D.error(w, 'a hint needs at least one line starting with -')
    ST.hints.append(h)


def p_sound(lines, n, rest, colon):
    w = n.where()
    m = re.match(r'^([a-z][a-z0-9_]*)\s+([a-z]+)\s*:?\s*(.*)$', rest.strip())
    if not m:
        D.error(w, 'sound <name> <instrument> <note> <frames>, <note> <frames>, ...')
        return
    name, inst, notes = m.group(1), m.group(2), m.group(3)
    if inst not in INSTRUMENTS:
        D.error(w, f'unknown instrument "{inst}"' + closest(inst, INSTRUMENTS) + f'; instruments: {", ".join(INSTRUMENTS)}')
        return
    seq = []
    for part in notes.split(','):
        p = part.split()
        if not p:
            continue
        if len(p) != 2:
            D.error(w, f'each note is <note> <frames>, as C4 20 or rest 10 (got {part.strip()!r})')
            continue
        frames = int_or(p[1], w, 1, 255)
        if p[0] == 'rest':
            seq.append((0, frames))
            continue
        mm = re.match(r'^([a-gA-G])([#b]?)(-?\d)$', p[0])
        if not mm:
            D.error(w, f'a note is a letter, # or b, and an octave, as C4, F#3, Bb2 (got {p[0]!r})')
            continue
        midi = 12 * (int(mm.group(3)) + 1) + NOTE_NAMES[mm.group(1).lower()] + {'#': 1, 'b': -1, '': 0}[mm.group(2)]
        hz = 440.0 * 2 ** ((midi - 69) / 12)
        seq.append((int(round(hz * 2 ** 32 / 48000)), frames))
    if not seq:
        D.error(w, 'a sound needs at least one note')
    if name in ST.sounds:
        D.error(w, f'sound "{name}" is already declared')
    ST.sounds[name] = (len(ST.sounds), INSTRUMENTS.index(inst), seq, w)


def p_picture(lines, n, rest, colon):
    ident = need_ident(rest.strip(), n.where(), 'a picture')
    if ident:
        if ident in ST.pictures:
            D.error(n.where(), f'picture "{ident}" is already declared')
        a = p_art(lines, n, relative=False)
        ST.pictures[ident] = (len(ST.pictures), a, n.where())


def p_achievement(lines, n, rest, colon):
    t = tokens(rest, n.where())
    if len(t) != 3 or not isinstance(t[1], tuple):
        D.error(n.where(), 'achievement <name> "title" <points>')
        return
    ST.achievements[t[0]] = (len(ST.achievements), t[1][1], int_or(t[2], n.where(), 0, 1000), n.where())


TOP = {
    'game': p_game, 'colour': p_colour, 'color': p_colour, 'flag': p_flag, 'counter': p_counter,
    'message': p_message, 'room': p_room, 'object': p_object, 'npc': p_npc, 'verb': p_verb,
    'on': p_rule('on'), 'after': p_rule('after'), 'event': p_event, 'hint': p_hint, 'sound': p_sound,
    'picture': p_picture, 'achievement': p_achievement, 'minutes': p_minutes,
}


# --- Resolving and compiling ------------------------------------------------


class Code:
    """The byte code of conditions and effects, in one array."""

    def __init__(self):
        self.b = bytearray([0])   # offset 0: an empty condition

    def add(self, bs):
        if not bs or bs == bytes([0]):
            return 0
        off = len(self.b)
        self.b += bs
        return off


CODE = Code()


class Texts:
    def __init__(self):
        self.items = []           # (encoded bytes)
        self.index = {}
        self.raw_bytes = 0
        self.long = []

    def add(self, s, where, check=True):
        if s is None or s == '':
            return 0
        enc = encode_text(s, where)
        if check:
            check_text_length(s, where)
        if enc in self.index:
            return self.index[enc]
        self.items.append(enc)
        self.index[enc] = len(self.items)   # ids from 1
        self.raw_bytes += len(enc) + 1
        return len(self.items)


TEXTS = Texts()

CP1252_EXTRA = {'€': 0x80, '‚': 0x82, '…': 0x85, '‘': 0x91, '’': 0x92, '“': 0x93,
                '”': 0x94, '•': 0x95, '–': 0x96, '—': 0x97, '−': 0x9F}


def encode_text(s, where):
    out = bytearray()
    i = 0
    while i < len(s):
        ch = s[i]
        if ch == '{':
            j = s.find('}', i)
            name = s[i + 1:j] if j > 0 else ''
            if name not in PH_CODE:
                D.error(where, f'unknown placeholder {{{name}}}' + closest(name, PH_CODE))
                i = j + 1 if j > 0 else len(s)
                continue
            out += bytes([2, PH_CODE[name]])
            i = j + 1
            continue
        o = ord(ch)
        if ch == '\n' or 0x20 <= o < 0x7F:
            out.append(o)
        elif 0xA0 <= o <= 0xFF:
            out += bytes([1, o])
        elif ch in CP1252_EXTRA:
            out += bytes([1, CP1252_EXTRA[ch]])
        else:
            D.error(where, f'the character {ch!r} (U+{o:04X}) is outside what the text area draws (ASCII, Latin-1, dashes, curly quotes, bullet, ellipsis)')
        i += 1
    return bytes(out)


def wrap_lines(s, width=TEXT_COLS):
    n = 0
    for para in re.sub(r'\{[^}]*\}', 'xxxxxxxxxx', s).split('\n'):
        words = para.split(' ')
        col = 0
        n += 1
        for wd in words:
            if col and col + 1 + len(wd) > width:
                n += 1
                col = len(wd)
            else:
                col += (1 if col else 0) + len(wd)
    return n


def check_text_length(s, where):
    n = wrap_lines(s)
    if n > 60:
        D.error(where, f'this text takes {n} lines; at most 60 (the text area shows {TEXT_ROWS} at a time)')
    elif n > TEXT_ROWS - 1:
        TEXTS.long.append((where, n))


def compress(items):
    """Byte-pair encoding: the most frequent pair of symbols becomes one new
    byte, over and over, using the bytes text never holds. An escape (byte 1
    or 2) and the byte after it stay as they are, never part of a pair, so
    the engine reads them raw. Returns (token table, encoded items)."""
    free = [b for b in range(3, 256) if b != 10 and not (0x20 <= b < 0x7F)]
    seqs = []
    for x in items:
        s, i = [], 0
        while i < len(x):
            if x[i] in (1, 2):
                s.append(0x10000 | x[i] << 8 | x[i + 1])
                i += 2
            else:
                s.append(x[i])
                i += 1
        seqs.append(s)
    table = []
    for tok in free:
        counts = {}
        for s in seqs:
            for a, b in zip(s, s[1:]):
                if a < 256 and b < 256:
                    counts[(a, b)] = counts.get((a, b), 0) + 1
        if not counts:
            break
        pair, cnt = max(counts.items(), key=lambda kv: (kv[1], -kv[0][0], -kv[0][1]))
        if cnt < 3:
            break
        table.append((tok, pair))
        for k, s in enumerate(seqs):
            out, i = [], 0
            while i < len(s):
                if i + 1 < len(s) and s[i] == pair[0] and s[i + 1] == pair[1]:
                    out.append(tok)
                    i += 2
                else:
                    out.append(s[i])
                    i += 1
            seqs[k] = out
    enc = []
    for s in seqs:
        b = bytearray()
        for v in s:
            if v > 0xFFFF:
                b += bytes([(v >> 8) & 255, v & 255])
            else:
                b.append(v)
        enc.append(bytes(b))
    return table, enc


def loc_code(tok, where, allow_player=True):
    if tok in ('nowhere', 'none'):
        return 0
    if tok in ('player', 'carried', 'held') and allow_player:
        return 1
    if tok == 'worn' and allow_player:
        return 2
    if tok in ST.rooms:
        return 3 + ST.rooms[tok].index
    if tok in ST.objs:
        return 3 + len(ST.rooms) + ST.objs[tok].index
    D.error(where, f'unknown place "{tok}"' + closest(tok, list(ST.rooms) + list(ST.objs) + ['nowhere', 'player', 'worn']))
    return 0


def room_ref(tok, where):
    if tok in ST.rooms:
        return ST.rooms[tok].index
    D.error(where, f'unknown room "{tok}"' + closest(str(tok), ST.rooms))
    return 0


def obj_ref(tok, where):
    if isinstance(tok, str) and tok in ST.objs:
        return ST.objs[tok].index
    D.error(where, f'unknown object "{tok}"' + closest(str(tok), ST.objs))
    return 0


def flag_ref(tok, where, use):
    if not isinstance(tok, str) or not re.match(r'^[a-z][a-z0-9_]*$', tok):
        D.error(where, f'expected a flag name, got {tok!r}')
        return 0
    i = declare_flag(tok, where)
    ST.flag_uses[tok][use] += 1
    return i


def counter_ref(tok, where):
    if tok in BUILTIN_COUNTERS:
        return BUILTIN_COUNTERS.index(tok)
    if tok in ST.counters:
        return ST.counters[tok][0]
    D.error(where, f'unknown counter "{tok}"' + closest(str(tok), BUILTIN_COUNTERS + list(ST.counters)))
    return 0


def compile_cond(toks, where):
    """Conditions joined by `and`. Returns the code bytes (ending in END)."""
    out = bytearray()
    parts, cur = [], []
    for t in toks:
        if t == 'and':
            parts.append(cur)
            cur = []
        elif t != ',':
            cur.append(t)
    parts.append(cur)
    for p in parts:
        if not p:
            D.error(where, 'an empty condition')
            continue
        neg = 0
        if p[0] == 'not':
            neg = 0x80
            p = p[1:]
        if not p:
            D.error(where, '"not" needs a condition after it')
            continue
        k = p[0]
        a = p[1:]

        def one(op, nargs=1):
            if len(a) != nargs:
                D.error(where, f'"{k}" takes {nargs} name{"s" if nargs > 1 else ""}')
                return False
            return True
        if len(p) == 1 and isinstance(k, str) and k not in ('dark', 'lit', 'beam'):
            out += bytes([C['FLAG'] | neg, flag_ref(k, where, 1)])
        elif k == 'in' and one('in'):
            out += bytes([C['IN'] | neg, room_ref(a[0], where)])
        elif k == 'carrying' and one(k):
            out += bytes([C['CARRYING'] | neg, obj_ref(a[0], where)])
        elif k == 'here' and one(k):
            out += bytes([C['HERE'] | neg, obj_ref(a[0], where)])
        elif k in ('open', 'closed') and one(k):
            out += bytes([C['OPEN'] | (neg ^ (0x80 if k == 'closed' else 0)), obj_ref(a[0], where)])
        elif k in ('locked', 'unlocked') and one(k):
            out += bytes([C['LOCKED'] | (neg ^ (0x80 if k == 'unlocked' else 0)), obj_ref(a[0], where)])
        elif k in ('on', 'off') and one(k):
            out += bytes([C['ON'] | (neg ^ (0x80 if k == 'off' else 0)), obj_ref(a[0], where)])
        elif k == 'worn' and one(k):
            out += bytes([C['WORN'] | neg, obj_ref(a[0], where)])
        elif k == 'visited' and one(k):
            out += bytes([C['VISITED'] | neg, room_ref(a[0], where)])
        elif k == 'dark' and not a:
            out += bytes([C['DARK'] | neg])
        elif k == 'lit' and not a:
            out += bytes([C['DARK'] | (neg ^ 0x80)])
        elif k == 'beam' and not a:
            out += bytes([C['BEAM'] | neg])
        elif k == 'hears' and one(k):
            out += bytes([C['HEARS'] | neg, obj_ref(a[0], where)])
        elif k == 'random' and one(k):
            out += bytes([C['RANDOM'] | neg, int_or(a[0], where, 1, 100)])
        elif len(p) == 3 and p[1] == 'in':
            out += bytes([C['OBJIN'] | neg, obj_ref(p[0], where), loc_code(p[2], where)])
        elif len(p) == 3 and p[1] in CMP_OPS:
            ci = counter_ref(p[0], where)
            if p[0] == 'time' and ':' in str(p[2]):
                v = parse_time(p[2], where)
            else:
                v = int_or(p[2], where, -32768, 32767)
            out += bytes([C['CMP'] | neg, ci, CMP_OPS.index(p[1])]) + struct.pack('<h', v)
        else:
            D.error(where, f'not a condition I know: {" ".join(x if isinstance(x, str) else repr(x[1]) for x in p)} '
                    '(flags, in ROOM, carrying OBJ, here OBJ, OBJ in PLACE, open/closed/locked/unlocked/on/off/worn OBJ, '
                    'visited ROOM, dark, lit, beam, hears NPC, random PERCENT, COUNTER >= N; join with and; not before any)')
    out.append(C['END'])
    return bytes(out)


def compile_conds(conds):
    toks = []
    where = None
    for t, w in conds:
        if toks:
            toks.append('and')
        toks += t
        where = where or w
    if not toks:
        return 0
    return CODE.add(compile_cond(toks, where))


def text_ref(i):
    return struct.pack('<I', i)[:3]


def compile_effects(nodes, lines):
    out = bytearray()
    for c in nodes:
        key, rest, colon = key_and_rest(c.text)
        w = c.where()
        t = [x for x in tokens(rest, w) if x != ','] if not colon else []
        if key == 'say':
            txt = block_text(lines, c, rest) if colon else (t[0][1] if t and isinstance(t[0], tuple) else rest)
            out += bytes([E['SAY']]) + text_ref(TEXTS.add(txt, w))
        elif key in ('set', 'clear', 'toggle'):
            if len(t) != 1:
                D.error(w, f'{key} <flag>')
                continue
            out += bytes([E[key.upper()], flag_ref(t[0], w, 0)])
        elif key == 'move':
            if len(t) != 3 or t[1] != 'to':
                D.error(w, 'move <object> to <room | object | player | worn | nowhere>')
                continue
            out += bytes([E['MOVE'], obj_ref(t[0], w), loc_code(t[2], w)])
            ST.moved.add(t[0])
        elif key == 'go':
            if len(t) != 1:
                D.error(w, 'go <room>')
                continue
            out += bytes([E['GO'], room_ref(t[0], w)])
            ST.go_targets.add(t[0])
        elif key == 'make':
            if len(t) != 2 or t[1] not in STATES:
                D.error(w, 'make <object> open | closed | locked | unlocked | on | off')
                continue
            out += bytes([E['STATE'], obj_ref(t[0], w), STATES.index(t[1])])
        elif key == 'exit':
            # exit <room> <dir> to <room> | exit <room> <dir> none
            dirn = None
            if len(t) >= 3:
                for k2, v in DIR_WORDS.items():
                    if t[1] in v or t[1] == k2:
                        dirn = k2
            if dirn is None or not (len(t) == 3 and t[2] == 'none' or len(t) == 4 and t[2] == 'to'):
                D.error(w, 'exit <room> <direction> to <room>, or exit <room> <direction> none')
                continue
            dest = 0xFF if t[2] == 'none' else room_ref(t[3], w)
            if t[2] != 'none':
                ST.go_targets.add(t[3])
            out += bytes([E['EXIT'], room_ref(t[0], w), DIRS.index(dirn), dest])
        elif key == 'sound':
            if len(t) != 1 or t[0] not in ST.sounds:
                D.error(w, f'unknown sound {t[0] if t else ""!r}' + (closest(t[0], ST.sounds) if t else ''))
                continue
            out += bytes([E['SOUND'], ST.sounds[t[0]][0]])
        elif key in ('add', 'let'):
            if len(t) != 2:
                D.error(w, f'{key} <counter> <number>')
                continue
            ci = counter_ref(t[0], w)
            if ci < 2 or ci in (2, 3):
                if t[0] != 'score':
                    D.error(w, f'"{t[0]}" cannot be changed')
            out += bytes([E[key.upper()], ci]) + struct.pack('<h', int_or(t[1], w, -32768, 32767))
        elif key == 'score':
            if len(t) != 1:
                D.error(w, 'score <points>')
                continue
            out += bytes([E['ADD'], 1]) + struct.pack('<h', int_or(t[0], w, -32768, 32767))
        elif key == 'schedule':
            if len(t) != 3 or t[1] != 'in':
                D.error(w, 'schedule <timer event> in <moves>')
                continue
            ev = ST.events.get(t[0])
            if not ev or ev.trigger != 'timer':
                D.error(w, f'"{t[0]}" is not a timer event' + closest(t[0], [e for e in ST.events if ST.events[e].trigger == 'timer']))
                continue
            out += bytes([E['SCHEDULE'], ev.index]) + struct.pack('<H', int_or(t[2], w, 1, 60000))
        elif key == 'cancel':
            ev = ST.events.get(t[0]) if t else None
            if not ev or ev.trigger != 'timer':
                D.error(w, 'cancel <timer event>')
                continue
            out += bytes([E['CANCEL'], ev.index])
        elif key in ('win', 'lose'):
            pic = 0xFF
            m = re.match(r'^([a-z][a-z0-9_]*)?\s*$', rest.split(':')[0] if colon else rest)
            pname = None
            if not colon and t and isinstance(t[0], str):
                pname = t[0]
            if colon:
                pname = None
            # win [picture]: text   -> key_and_rest put the picture name before the colon
            mm = re.match(r'^(win|lose)\s+([a-z][a-z0-9_]*)\s*:\s*(.*)$', c.text)
            txt = ''
            if mm:
                pname = mm.group(2)
                txt = block_text(lines, c, mm.group(3))
            elif colon:
                txt = block_text(lines, c, rest)
            elif t and isinstance(t[-1], tuple):
                txt = t[-1][1]
                pname = t[0] if isinstance(t[0], str) else None
            del m
            if pname:
                if pname not in ST.pictures:
                    D.error(w, f'unknown picture "{pname}"' + closest(pname, ST.pictures))
                else:
                    pic = ST.pictures[pname][0]
            out += bytes([E['END_GAME'], 1 if key == 'win' else 2, pic]) + text_ref(TEXTS.add(txt, w))
            ST.endings.add(key)
        elif key == 'loud':
            out += bytes([E['LOUD']])
        elif key == 'continue':
            out += bytes([E['CONTINUE']])
        elif key == 'look':
            out += bytes([E['LOOK']])
        elif key == 'achieve':
            if len(t) != 1 or t[0] not in ST.achievements:
                D.error(w, 'achieve <achievement>' + (closest(t[0], ST.achievements) if t else ''))
                continue
            out += bytes([E['ACHIEVE'], ST.achievements[t[0]][0]])
        elif key == 'try':
            # try <verb or action> [<object> [<object>]]: that command instead
            if not t or len(t) > 3:
                D.error(w, 'try <verb> [<object> [<second object>]]')
                continue
            ai = ST.action_index.get(t[0])
            if ai is None and t[0] in ST.W.w and ST.W.w[t[0]]['verb'] != 0xFF:
                ai = ST.W.w[t[0]]['verb']
            if ai is None:
                D.error(w, f'unknown verb "{t[0]}"' + closest(t[0], list(ST.action_index)))
                continue
            o1 = obj_ref(t[1], w) if len(t) > 1 else 0xFF
            o2 = obj_ref(t[2], w) if len(t) > 2 else 0xFF
            out += bytes([E['TRY'], ai, o1, o2])
        elif key == 'minutes':
            if len(t) != 1:
                D.error(w, 'minutes <n>: what this move takes')
                continue
            out += bytes([E['MINUTES']]) + struct.pack('<H', int_or(t[0], w, 0, 600))
        elif key == 'aim':
            if len(t) != 1:
                D.error(w, 'aim <object>')
                continue
            out += bytes([E['AIM'], obj_ref(t[0], w)])
        elif key == 'if':
            D.error(w, 'put the `if` lines of a rule before its effects')
        else:
            D.error(w, f'unknown effect "{key}"' + closest(key, ['say', 'set', 'clear', 'toggle', 'move', 'go', 'make', 'exit', 'sound', 'add', 'let', 'score', 'schedule', 'cancel', 'win', 'lose', 'loud', 'continue', 'look', 'achieve', 'aim']))
    out.append(E['END'])
    return CODE.add(bytes(out))


# --- Words -------------------------------------------------------------------


class Words:
    def __init__(self):
        self.w = {}    # word -> dict(kinds, verb, dir, prep)

    def get(self, word):
        word = word.lower()
        if word not in self.w:
            self.w[word] = {'kinds': 0, 'verb': 0xFF, 'dir': 0xFF, 'prep': 0}
        return self.w[word]

    def add(self, word, kind, **kv):
        e = self.get(word)
        e['kinds'] |= kind
        for k, v in kv.items():
            if k == 'verb' and e['verb'] != 0xFF and e['verb'] != v:
                return e, False
            e[k] = v
        return e, True


def resolve():
    for i, r in enumerate(ST.room_order):
        r.index = i
    for i, o in enumerate(ST.obj_order):
        o.index = i
    ST.moved, ST.go_targets, ST.endings = set(), set(), set()
    for i, e in enumerate(ST.event_order):
        e.index = i
    if len(ST.rooms) == 0:
        raise StoryError('the story has no rooms')
    if 3 + len(ST.rooms) + len(ST.objs) > 255:
        D.error('-', f'{len(ST.rooms)} rooms and {len(ST.objs)} objects: at most 252 together')
    if not ST.start:
        D.error('-', 'game: start <room> is missing')
        start = 0
    else:
        start = room_ref(ST.start[0], ST.start[1])
    ST.start_index = start


def emit(out_path, check_only=False, stats=False):
    W = Words()
    nwords_err = []
    for name, words in DIR_WORDS.items():
        for wd in words:
            W.add(wd, WK_DIR, dir=DIRS.index(name))
    for pid, (name, words) in enumerate(PREPS, 1):
        for wd in words:
            W.add(wd, WK_PREP, prep=pid)
    for a in ARTICLES:
        W.add(a, WK_ART)
    for a in PRONOUNS:
        W.add(a, WK_IT)
    for a in ALLS:
        W.add(a, WK_ALL)
    for a in SEPARATORS:
        W.add(a, WK_SEP)
    for i, (name, flags, words, dm) in enumerate(ACTIONS):
        for wd in words:
            W.add(wd, WK_VERB, verb=i)
    for wd in ['pick', 'switch', 'speak', 'blow', 'get', 'put', 'take', 'turn', 'look', 'talk', 'listen', 'light', 'climb', 'shine', 'point', 'aim']:
        W.get(wd)
    W.add('pick', WK_VERB, verb=ACTION_ID['take'])
    W.add('speak', WK_VERB, verb=ACTION_ID['talk'])
    # Story verbs.
    actions = [list(a) for a in ACTIONS]
    for name in ST.custom_actions:
        actions.append([name, NOUN, [], None])
    action_index = {a[0]: i for i, a in enumerate(actions)}
    ST.action_index, ST.W = action_index, W
    custom_default = {}
    for action, words, default, flags, where in ST.verbs:
        ai = action_index[action]
        if ai >= len(ACTIONS):
            actions[ai][1] = flags
            if default:
                custom_default[ai] = default
        elif default:
            D.warn(where, f'"default" is ignored for the built-in action {action}; use message {ACTIONS[ai][3] or action}: ...')
        for wd in words:
            parts = wd.split()
            if len(parts) == 1:
                e, ok = W.add(wd, WK_VERB, verb=ai)
                if not ok:
                    D.warn(where, f'"{wd}" was already the verb {actions[e["verb"]][0]}; it is now {action}')
                    e['verb'] = ai
            elif len(parts) == 2:
                W.add(parts[0], WK_VERB)
                if W.get(parts[0])['verb'] == 0xFF:
                    W.get(parts[0])['verb'] = ai
                PARTICLES.append((parts[0], parts[1], action, 1))
                W.get(parts[1])
            else:
                D.error(where, f'a verb is one or two words: "{wd}"')
    # Object words.
    for o in ST.obj_order:
        if not o.nouns:
            o.nouns = [o.id.split('_')[-1]]
        for wd in o.nouns:
            W.add(wd, WK_NOUN)
        for wd in o.adjs:
            W.add(wd, WK_ADJ)
        for ws, txt, where in o.topics:
            for wd in ws:
                W.add(wd, WK_NOUN)
    for wd in W.w:
        if not re.match(r'^[a-z0-9\'-]{1,15}$', wd):
            nwords_err.append(wd)
    for wd in nwords_err:
        D.error('-', f'the word "{wd}" must be 1 to 15 letters, digits, hyphens or apostrophes')
    words_sorted = sorted(W.w)
    wid = {wd: i for i, wd in enumerate(words_sorted)}
    if len(words_sorted) > 4000:
        D.error('-', 'more than 4000 words')

    # Messages.
    msg_keys = list(MESSAGES)
    if ST.help:
        ST.messages['help'] = ST.help
    msg_ids = [TEXTS.add(ST.messages[k], f'message {k}', check=False) for k in msg_keys]

    # Flags used by layers' reveal.
    # Rooms.
    rooms_c = []
    exits_b = bytearray()
    notes_b = bytearray()
    for r in ST.room_order:
        r.exits_off = len(exits_b)
        for e in r.exits:
            w = e['where']
            dest = room_ref(e['to'], w) if e['to'] else 0xFF
            door = obj_ref(e['door'], w) if e['door'] else 0xFF
            if e['door'] and e['door'] in ST.objs and 'openable' not in ST.objs[e['door']].kinds:
                D.error(w, f'the door "{e["door"]}" must be openable (give it `door` or `openable`)')
            cond = CODE.add(compile_cond(e['cond'], w)) if e['cond'] else 0
            refuse = TEXTS.add(e['refuse'], w) if e['refuse'] else 0
            exits_b += bytes([DIRS.index(e['dir']), dest, door]) + struct.pack('<H', cond) + text_ref(refuse)
        exits_b += bytes([0xFF])
        r.notes_off = len(notes_b)
        for toks, w, txt in r.notes:
            notes_b += struct.pack('<H', CODE.add(compile_cond(toks, w))) + text_ref(TEXTS.add(txt, w))
        notes_b += b'\0\0'
    # Art.
    art_b = bytearray([0])
    room_ink = {}

    def add_art(art, ident, where, relative):
        if art is None:
            return 0
        off = len(art_b)
        art_b.append(len(art.layers))
        npts = 0
        ink = 0
        for L in art.layers:
            cond = CODE.add(compile_cond(L.cond[0], L.cond[1])) if L.cond else 0
            reveal = flag_ref(L.reveal[0], L.reveal[1], 0) if L.reveal else 0xFF
            rtext = TEXTS.add(L.reveal_text, L.where) if L.reveal_text else 0
            xs = [p[0] for s, w in L.shapes for p in s]
            ys = [p[1] for s, w in L.shapes for p in s]
            if L.anchor:
                ax, ay = L.anchor
            elif xs:
                ax, ay = (min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2
            else:
                ax = ay = 0
            ltype = (L_HIDDEN if L.hidden else L_NORMAL) | (4 if L.faint else 0)
            an, a1, a2, a3, a4 = L.anim
            if an == A_DRIFT:
                a3 = max(8, min(2000, a3)) // 8
            art_b.extend(bytes([ltype]) + bytes(L.colour) + bytes([an, a1 & 255, a2 & 255, a3 & 255, a4 & 255, reveal])
                         + text_ref(rtext) + pack_point(ax, ay) + struct.pack('<HHH', cond, len(L.shapes), ST.hidden_count if L.hidden else 0))
            if L.hidden:
                ST.hidden_count += len(L.shapes)
            for pts, w in L.shapes:
                for (x, y) in pts:
                    dx = abs(L.anim[1]) if an == A_DRIFT else 0
                    dy = abs(L.anim[2]) if an == A_DRIFT else 0
                    if relative:
                        if not (-200 <= x <= 200 and -200 <= y <= 200):
                            D.error(w, f'a figure\'s points are within 200 units of its position (got {x:g}, {y:g})')
                    elif not (dx <= x <= SCENE_W - dx and dy <= y <= SCENE_H - dy):
                        D.error(w, f'point ({x:g}, {y:g}) is outside the picture (0 to {SCENE_W} by 0 to {SCENE_H}' +
                                (', less the drift' if dx or dy else '') + ')')
                if len(pts) > 255:
                    D.error(w, 'a shape has more than 255 points; split it')
                    pts = pts[:255]
                art_b.append(len(pts))
                for (x, y) in pts:
                    art_b.extend(pack_point(x, y))
                npts += len(pts)
                ink += shape_ink(pts)
        if npts > MAX_ROOM_POINTS:
            D.error(where, f'{ident}: {npts} points; at most {MAX_ROOM_POINTS} in one picture')
        return off, ink

    rooms_art = {}
    for r in ST.room_order:
        res = add_art(r.art, r.id, r.where, False) if r.art else (0, 0)
        rooms_art[r.id] = res[0]
        room_ink[r.id] = res[1]
    objs_art = {}
    obj_ink = {}
    for o in ST.obj_order:
        res = add_art(o.figure, o.id, o.where, True) if o.figure else (0, 0)
        objs_art[o.id] = res[0]
        obj_ink[o.id] = res[1]
    pics = []
    for name, (i, a, w) in sorted(ST.pictures.items(), key=lambda kv: kv[1][0]):
        res = add_art(a, name, w, False)
        pics.append(res[0])
        room_ink['picture ' + name] = res[1]
    # Worst ink per room: its art, the figures that start there, the beam.
    beam_ink = shape_ink([(math.cos(a / 12 * math.pi * 2) * ST.beam + 320, math.sin(a / 12 * math.pi * 2) * ST.beam + 128) for a in range(13)]) * 2 + 2 * shape_ink([(320, 256), (320, 100)])
    frame_ink = shape_ink([(0, 0), (640, 0), (640, 256), (0, 256), (0, 0)])
    worst_room, worst = None, 0
    for r in ST.room_order:
        ink = room_ink[r.id] + beam_ink + frame_ink
        for o in ST.obj_order:
            if o.loc and o.loc[0] == r.id:
                ink += obj_ink[o.id]
        # Hidden and revealed layers draw twice at most (faint and in the beam).
        if ink > worst:
            worst, worst_room = ink, r.id
        if ink > INK_ERROR:
            D.error(r.where, f'{r.id}: the picture may draw {ink} square units of ink; at most {INK_ERROR} (half the device\'s cap)')
    ST.worst_ink = (worst_room, worst)

    # Objects.
    wl = [0]  # word lists: count, ids
    objs_c = []
    topics_b = bytearray()
    idle_b = bytearray()
    for o in ST.obj_order:
        w = o.where
        if o.loc is None:
            loc = 0
        else:
            loc = loc_code(o.loc[0], o.loc[1])
            if o.loc[0] == 'worn':
                if 'wearable' not in o.kinds:
                    D.error(o.loc[1], f'{o.id} is worn but not wearable')
        also = room_ref(o.also[0], o.also[1]) + 3 if o.also else 0
        kinds = 0
        for k in o.kinds:
            kinds |= K[k]
        if o.portable:
            kinds |= K['portable']
        key = obj_ref(o.key[0], o.key[1]) if o.key else 0xFF
        if 'lockable' in o.kinds and not o.key:
            D.warn(w, f'{o.id} is lockable but has no key: only rules can unlock it')
        if 'switchable' not in o.kinds and o.state & S_ON and 'light' not in o.kinds:
            pass
        art = {'a': 0, 'an': 1, 'some': 2, 'the': 3, 'none': 4}
        article = o.article
        if article is None:
            if 'proper' in o.kinds:
                article = 'none'
            elif o.npc:
                article = 'the'
            elif 'plural' in o.kinds:
                article = 'some'
            else:
                article = 'an' if o.name[:1].lower() in 'aeiou' else 'a'
        nouns_off = len(wl)
        wl.append(len(o.nouns))
        wl.extend(wid[x] for x in o.nouns)
        adjs_off = len(wl)
        wl.append(len(o.adjs))
        wl.extend(wid[x] for x in o.adjs)
        t_off = len(topics_b)
        for ws, txt, tw in o.topics:
            topics_b += bytes([len(ws)]) + b''.join(struct.pack('<H', wid[x]) for x in ws) + text_ref(TEXTS.add(txt, tw))
        topics_b += b'\0'
        i_off = len(idle_b)
        every, texts = o.idle
        idle_b += bytes([every, len(texts)]) + b''.join(text_ref(TEXTS.add(x, w)) for x in texts)
        if o.at:
            ax, ay = o.at
        else:
            ax, ay = (-1, -1)
        if o.figure and not o.at:
            D.error(w, f'{o.id} has a figure but no position: give it `at x y`')
        top = o.top or (0, -24)
        if o.at and not (0 <= ax <= SCENE_W and 0 <= ay <= SCENE_H):
            D.error(w, f'{o.id}: position ({ax:g}, {ay:g}) is outside the picture')
        objs_c.append(dict(
            name=TEXTS.add(o.name, w), article=art[article], kinds=kinds, loc=loc, also=also, state=o.state, key=key,
            desc=TEXTS.add(o.desc, w), initial=TEXTS.add(o.initial, w), read=TEXTS.add(o.readable, w),
            here=TEXTS.add(o.here, w), talk=TEXTS.add(o.talk, w), unknown=TEXTS.add(o.unknown, w),
            arrives=TEXTS.add(o.arrives, w), leaves=TEXTS.add(o.leaves, w),
            nouns=nouns_off, adjs=adjs_off, topics=t_off, idle=i_off,
            x=int(round(ax * 2)) if o.at else -1, y=int(round(ay * 2)) if o.at else -1,
            tx=int(round(top[0] * 2)), ty=int(round(top[1] * 2)), art=objs_art[o.id]))
        if o.npc and o.route:
            for mv, place, rw in o.route:
                when = parse_time(mv, rw) if ':' in mv else None
                ev = Event(f'_route_{o.id}_{mv}', rw)
                ev.trigger = 'at'
                ev.raw = mv
                ev.lines = []
                ev.cond = []
                ev.effects = []
                ev.route = (o.id, place)
                ev.index = len(ST.event_order)
                ST.event_order.append(ev)
                ST.events[ev.id] = ev
                del when
    if ST.carry and ST.carry < 1:
        D.error('-', 'carry must be at least 1')

    for r in ST.room_order:
        rooms_c.append(dict(name=TEXTS.add(r.name, r.where), desc=TEXTS.add(r.desc, r.where), dark=1 if r.dark else 0,
                            exits=r.exits_off, notes=r.notes_off, art=rooms_art[r.id],
                            amb=ST.sounds[r.ambient[0]][0] if r.ambient and r.ambient[0] in ST.sounds else 0xFF,
                            amb_every=r.ambient[1] if r.ambient else 0,
                            spots=r.spots))
        if r.ambient and r.ambient[0] not in ST.sounds:
            D.error(r.ambient[2], f'unknown sound "{r.ambient[0]}"' + closest(r.ambient[0], ST.sounds))
    spots_b = bytearray()
    for rc in rooms_c:
        rc['spots_off'] = len(spots_b)
        spots_b.append(len(rc['spots']))
        for x, y, w in rc['spots']:
            if not (0 <= x <= SCENE_W and 0 <= y <= SCENE_H):
                D.error(w, 'spot is outside the picture')
            spots_b.extend(pack_point(x, y))

    # Rules.
    rules_c = []
    for r in ST.rules:
        t = r.head
        w = r.where
        verb = t[0]
        ai = None
        if verb in action_index:
            ai = action_index[verb]
        elif verb in W.w and W.w[verb]['verb'] != 0xFF:
            ai = W.w[verb]['verb']
        elif verb in DIRS or any(verb in v for v in DIR_WORDS.values()):
            ai = ACTION_ID['go']
            t = ['go'] + t
        else:
            D.error(w, f'unknown verb or action "{verb}"' + closest(verb, list(action_index) + list(W.w)))
            continue
        o1 = o2 = 0xFF
        prep = 0
        rest = t[1:]
        if ai == ACTION_ID['go']:
            if rest:
                d = rest[0]
                dn = None
                for k2, v in DIR_WORDS.items():
                    if d == k2 or d in v:
                        dn = k2
                if dn is None and d != 'any':
                    D.error(w, f'on go <direction>: unknown direction "{d}"')
                    continue
                o1 = 0xFE if d == 'any' else DIRS.index(dn)
            else:
                o1 = 0xFE
        else:
            # Particles written in the rule: "on look at box", "on turn on lantern"
            if rest and isinstance(rest[0], str) and rest[0] in W.w and W.w[rest[0]]['kinds'] & WK_PREP and rest[0] not in ST.objs:
                for vb, pt, act, tr in PARTICLES:
                    if vb == verb and pt == rest[0]:
                        ai = action_index[act]
                        rest = rest[1:]
                        break
            if rest:
                o1 = 0xFE if rest[0] == 'any' else obj_ref(rest[0], w)
                if len(rest) >= 3:
                    pw = rest[1]
                    pe = W.w.get(pw)
                    if not pe or not pe['kinds'] & WK_PREP:
                        D.error(w, f'expected a preposition, got {pw!r}')
                    else:
                        prep = pe['prep']
                    o2 = 0xFE if rest[2] == 'any' else obj_ref(rest[2], w)
                    for a1, p1, a2 in PREP_SWITCH:
                        if ai == action_index[a1] and PREP_ID[p1] == prep:
                            ai = action_index[a2]
                elif len(rest) == 2:
                    # An action's name with both objects: on put_in flask lamp.
                    o2 = 0xFE if rest[1] == 'any' else obj_ref(rest[1], w)
            else:
                o1 = 0xFD   # no object
        rules_c.append(dict(kind=0 if r.kind == 'on' else 1, action=ai, o1=o1, o2=o2,
                            cond=compile_conds(r.conds), eff=compile_effects(r.effects, r.lines)))

    # Events.
    events_c = []
    warn_events = []
    timers = 0
    for ev in ST.event_order:
        w = ev.where
        a = b = 0
        cond = 0
        kind = ev.trigger
        raw = getattr(ev, 'raw', '')
        if kind == 'at' and ':' in raw:
            kind = 'attime'
            a = minute_of(raw, w)
        elif kind == 'at':
            a = int_or(raw.strip(), w, 0, 60000)
        elif kind == 'every':
            m = re.match(r'^(\d+)(\s+minutes?)?(?:\s+from\s+(\S+))?$', raw.strip())
            if not m:
                D.error(w, 'every <moves> [from <move>], or every <n> minutes [from <time>]')
            else:
                a = int(m.group(1))
                if a < 1:
                    D.error(w, 'every 0')
                if m.group(2):
                    kind = 'everytime'
                    b = minute_of(m.group(3), w) if m.group(3) else a
                else:
                    b = int_or(m.group(3), w, 0, 60000) if m.group(3) else a
        elif kind in ('when', 'whenever'):
            cond = CODE.add(compile_cond(tokens(raw, w), w))
        elif kind == 'timer':
            a = timers
            timers += 1
        elif kind == 'enter':
            a = room_ref(raw.strip(), w)
        elif kind == 'noise':
            pass
        if ev.cond:
            c2 = compile_conds(ev.cond)
            if cond:
                # Both: the trigger's and the if lines'; join the code.
                D.error(w, 'a when/whenever event takes its condition on the first line only')
            cond = c2
        if hasattr(ev, 'route'):
            oid, place = ev.route
            eff = CODE.add(bytes([E['MOVE'], obj_ref(oid, w), loc_code(place, w)]) + bytes([E['END']]))
            ST.moved.add(oid)
        else:
            eff = compile_effects(ev.effects, ev.lines)
        events_c.append(dict(kind=T.get(kind, 0), a=a, b=b, cond=cond, eff=eff))
        if getattr(ev, 'warn', None):
            if kind != 'attime':
                D.error(w, 'warn needs an event at a time: event x at 0:15 warn 30 10')
            wt = getattr(ev, 'warning', None)
            if not wt:
                D.error(w, 'warn needs a `warning:` text ({left} is the minutes left)')
            else:
                for left in ev.warn:
                    txt = wt[0].replace('{left}', str(left))
                    weff = CODE.add(bytes([E['SAY']]) + text_ref(TEXTS.add(txt, wt[1])) + bytes([E['END']]))
                    warn_events.append(dict(kind=T['attime'], a=max(0, a - left), b=0, cond=0, eff=weff))
        ev.timer_index = a if kind == 'timer' else 0xFF
    events_c += warn_events

    # Hints.
    hints_c = []
    for h in ST.hints:
        cond = compile_conds(h['cond'])
        hints_c.append(dict(cond=cond, texts=[TEXTS.add(t, w) for t, w in h['texts']]))

    # Particles and prep switches (for the words they use).
    parts_c = []
    for vb, pt, act, tr in PARTICLES:
        if vb in wid and pt in wid:
            parts_c.append((wid[vb], wid[pt], action_index[act], tr))
    pswitch_c = [(action_index[a1], PREP_ID[p], action_index[a2]) for a1, p, a2 in PREP_SWITCH]

    # Checks across the story.
    check_story(rooms_c)

    # Status line and intro.
    status_id = TEXTS.add(ST.status, 'game status', check=False)
    intro_id = TEXTS.add(ST.intro, 'game intro')
    title_id = TEXTS.add(ST.title, 'game title', check=False)
    ach_c = [(TEXTS.add(title, w), pts) for name, (i, title, pts, w) in sorted(ST.achievements.items(), key=lambda kv: kv[1][0])]
    custom_c = [TEXTS.add(custom_default.get(i, ''), 'verb default') for i in range(len(ACTIONS), len(actions))]
    max_score = ST.max_score or sum(p for t, p in ach_c)

    # Flags: initial values.
    nflags = len(ST.flags)
    if nflags > 250:
        D.error('-', f'{nflags} flags; at most 250')
    flag_init = bytearray((nflags + 7) // 8 or 1)
    for name, (i, init, w) in ST.flags.items():
        if init:
            flag_init[i // 8] |= 1 << (i % 8)
    for name, (sets, tests) in ST.flag_uses.items():
        w = ST.flags[name][2]
        init = ST.flags[name][1]
        if tests and not sets and not init:
            D.warn(w, f'flag "{name}" is tested but never set')
        if sets and not tests:
            D.warn(w, f'flag "{name}" is set but never tested')

    # Sounds.
    snd_b = bytearray()
    snd_offs = []
    for name, (i, inst, seq, w) in sorted(ST.sounds.items(), key=lambda kv: kv[1][0]):
        snd_offs.append(len(snd_b))
        snd_b += bytes([inst, len(seq)])
        for inc, fr in seq:
            snd_b += struct.pack('<IB', inc, fr)

    # Texts last (everything has added its texts).
    table, enc = compress(TEXTS.items)
    text_b = bytearray([0])
    text_off = [0]
    for e in enc:
        text_off.append(len(text_b))
        text_b += e + b'\0'
    if len(text_b) >= 1 << 24:
        D.error('-', 'text over 16 MiB')
    if len(CODE.b) >= 1 << 16:
        D.error('-', f'the rules\' code is {len(CODE.b)} bytes; at most 65,535')
    if len(TEXTS.items) >= 1 << 16:
        D.error('-', 'more than 65,535 texts')
    if ST.step and ST.step[0] not in ST.sounds:
        D.error(ST.step[1], f'unknown sound "{ST.step[0]}"' + closest(ST.step[0], ST.sounds))

    if D.errors:
        return None

    counters_init = [0] * len(BUILTIN_COUNTERS) + [v for k, (i, v) in sorted(ST.counters.items(), key=lambda kv: kv[1][0])]
    clock = ST.clock or (0, 0, False)
    sizes = dict(text=len(text_b) + len(table) * 2 + 4 * len(text_off), text_raw=TEXTS.raw_bytes, art=len(art_b), code=len(CODE.b),
                 words=sum(len(x) + 1 for x in words_sorted) + 6 * len(words_sorted))
    lines = []
    P = lines.append
    P('/* Generated by story.py from the story files: do not edit. */')
    P(f'#define STORY_ID 0x{zlib.crc32(bytes(text_b) + bytes(CODE.b) + bytes(art_b)) & 0xFFFFFFFF:08X}u')
    P(f'#define N_ROOMS {len(rooms_c)}')
    P(f'#define N_OBJS {len(objs_c)}')
    P(f'#define N_WORDS {len(words_sorted)}')
    P(f'#define N_ACTIONS {len(actions)}')
    P(f'#define N_BUILTIN_ACTIONS {len(ACTIONS)}')
    P(f'#define N_FLAGS {max(1, nflags)}')
    P(f'#define N_COUNTERS {len(counters_init)}')
    P(f'#define N_EVENTS {max(1, len(events_c))}')
    P(f'#define N_EVENTS_REAL {len(events_c)}')
    P(f'#define N_TIMERS {max(1, timers)}')
    P(f'#define N_RULES {len(rules_c)}')
    P(f'#define N_HINTS {max(1, len(hints_c))}')
    P(f'#define N_HINTS_REAL {len(hints_c)}')
    P(f'#define N_SOUNDS {len(snd_offs)}')
    P(f'#define N_PICTURES {len(pics)}')
    P(f'#define N_ACHIEVEMENTS {max(1, len(ach_c))}')
    P(f'#define N_ACHIEVEMENTS_REAL {len(ach_c)}')
    P(f'#define N_PARTICLES {len(parts_c)}')
    P(f'#define N_PSWITCH {len(pswitch_c)}')
    P(f'#define N_TOKENS {len(table)}')
    P(f'#define START_ROOM {ST.start_index}')
    P(f'#define CARRY_LIMIT {ST.carry}')
    P(f'#define BEAM_R {ST.beam}')
    step = ST.sounds[ST.step[0]][0] if ST.step else 0xFF
    P(f'#define STEP_SOUND {step}')
    P(f'#define MAX_SCORE {max_score}')
    P(f'#define CLOCK_START {clock[0]}')
    P(f'#define CLOCK_STEP {clock[1]}')
    P(f'#define CLOCK_24H {1 if clock[2] else 0}')
    P(f'#define TEXT_STATUS {status_id}')
    P(f'#define TEXT_INTRO {intro_id}')
    P(f'#define TEXT_TITLE {title_id}')
    for i, k in enumerate(msg_keys):
        P(f'#define M_{k.upper()} {msg_ids[i]}')
    for i, a in enumerate(actions):
        P(f'#define A_{a[0].upper()} {i}')
    P('')
    P(f'static const uint8_t action_flags[N_ACTIONS] = {{{", ".join(str(a[1]) for a in actions)}}};')
    free = {'look', 'examine', 'inventory', 'read', 'search', 'again', 'undo', 'save', 'restore', 'restart', 'quit', 'help', 'hint', 'score', 'yes', 'no'}
    mins = []
    for a in actions:
        m = 0 if a[0] in free else ST.default_minutes
        mins.append(m)
    for name, (m, where) in ST.minutes.items():
        ai = action_index.get(name)
        if ai is None and name in W.w and W.w[name]['verb'] != 0xFF:
            ai = W.w[name]['verb']
        if ai is None:
            D.error(where, f'unknown action or verb "{name}"' + closest(name, list(action_index)))
            continue
        mins[ai] = m
    P(f'static const uint16_t action_minutes[N_ACTIONS] = {{{", ".join(map(str, mins))}}};')
    P(f'#define N_HIDDEN {max(1, ST.hidden_count)}')
    defaults = []
    for i, a in enumerate(actions):
        if i < len(ACTIONS):
            defaults.append(msg_ids[msg_keys.index(a[3])] if a[3] else 0)
        else:
            defaults.append(custom_c[i - len(ACTIONS)] or msg_ids[msg_keys.index('custom_default')])
    P(f'static const uint32_t action_default[N_ACTIONS] = {{{", ".join(map(str, defaults))}}};')
    P(f'static const uint8_t text_tokens[{max(1, len(table))}][2] = {{{", ".join(f"{{{a},{b}}}" for t, (a, b) in table) or "{0,0}"}}};')
    P(f'#define TOKEN_FIRST {table[0][0] if table else 255}')
    # Token bytes are not contiguous: a 256-entry map from byte to token index (255: not a token).
    tokmap = [255] * 256
    for k, (t, pair) in enumerate(table):
        tokmap[t] = k
    P(f'static const uint8_t token_of[256] = {{{", ".join(map(str, tokmap))}}};')
    P(f'static const uint32_t text_off[{len(text_off)}] = {{{", ".join(map(str, text_off))}}};')
    P(f'static const uint8_t text_data[{len(text_b)}] = {{{c_bytes(text_b)}}};')
    word_chars = bytearray()
    winfo = []
    for wd in words_sorted:
        e = W.w[wd]
        winfo.append((len(word_chars), len(wd), e['kinds'], e['verb'], e['dir'], e['prep']))
        word_chars += wd.encode()
    P(f'static const char word_chars[{len(word_chars)}] = {{{c_bytes(word_chars)}}};')
    P('static const struct word words[N_WORDS] = {' + ', '.join(f'{{{a},{b},{c},{d},{e},{f}}}' for a, b, c, d, e, f in winfo) + '};')
    P(f'static const uint16_t word_lists[{len(wl)}] = {{{", ".join(map(str, wl))}}};')
    P(f'static const uint16_t particle_tab[{max(1, len(parts_c))}][3] = {{{", ".join("{%d,%d,%d}" % (a, b, c | d << 8) for a, b, c, d in parts_c) or "{0}"}}};')
    P(f'static const uint8_t pswitch_tab[{max(1, len(pswitch_c))}][3] = {{{", ".join("{%d,%d,%d}" % t for t in pswitch_c) or "{0}"}}};')
    P(f'static const uint8_t code[{len(CODE.b)}] = {{{c_bytes(CODE.b)}}};')
    P(f'static const uint8_t exits_data[{len(exits_b)}] = {{{c_bytes(exits_b)}}};')
    P(f'static const uint8_t notes_data[{len(notes_b)}] = {{{c_bytes(notes_b)}}};')
    P(f'static const uint8_t spots_data[{len(spots_b)}] = {{{c_bytes(spots_b)}}};')
    P(f'static const uint8_t art_data[{len(art_b)}] = {{{c_bytes(art_b)}}};')
    P(f'static const uint8_t topics_data[{max(1, len(topics_b))}] = {{{c_bytes(topics_b) or "0"}}};')
    P(f'static const uint8_t idle_data[{max(1, len(idle_b))}] = {{{c_bytes(idle_b) or "0"}}};')
    P(f'static const uint8_t sound_data[{max(1, len(snd_b))}] = {{{c_bytes(snd_b) or "0"}}};')
    P(f'static const uint16_t sound_off[{max(1, len(snd_offs))}] = {{{", ".join(map(str, snd_offs)) or "0"}}};')
    P(f'static const uint32_t picture_art[{max(1, len(pics))}] = {{{", ".join(map(str, pics)) or "0"}}};')
    P('static const struct room rooms[N_ROOMS] = {' + ', '.join(
        '{%d,%d,%d,%d,%d,%d,%d,%d,%d}' % (r['name'], r['desc'], r['art'], r['exits'], r['notes'], r['spots_off'], r['dark'], r['amb'], r['amb_every'])
        for r in rooms_c) + '};')
    P('static const struct object objs[N_OBJS] = {' + ', '.join(
        '{%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d}' % (
            o['name'], o['desc'], o['initial'], o['read'], o['here'], o['talk'], o['unknown'], o['arrives'], o['leaves'],
            o['art'], o['kinds'], o['nouns'], o['adjs'], o['topics'], o['idle'], o['x'], o['y'], o['tx'], o['ty'],
            o['loc'], o['also'], o['state'], o['key'], o['article'], 0)
        for o in objs_c) + '};')
    P('static const struct rule rules[' + str(max(1, len(rules_c))) + '] = {' + (', '.join(
        '{%d,%d,%d,%d,%d,%d}' % (r['cond'], r['eff'], r['kind'], r['action'], r['o1'], r['o2']) for r in rules_c) or '{0}') + '};')
    P('static const struct event events[N_EVENTS] = {' + (', '.join(
        '{%d,%d,%d,%d,%d}' % (e['cond'], e['eff'], e['a'], e['b'], e['kind']) for e in events_c) or '{0}') + '};')
    hint_texts = []
    hints_rows = []
    for h in hints_c:
        hints_rows.append('{%d,%d,%d}' % (h['cond'], len(hint_texts), len(h['texts'])))
        hint_texts += h['texts']
    P('static const struct hint hints[N_HINTS] = {' + (', '.join(hints_rows) or '{0}') + '};')
    P(f'static const uint32_t hint_texts[{max(1, len(hint_texts))}] = {{{", ".join(map(str, hint_texts)) or "0"}}};')
    P(f'static const uint32_t achievement_text[N_ACHIEVEMENTS] = {{{", ".join(str(t) for t, p in ach_c) or "0"}}};')
    P(f'static const int16_t achievement_points[N_ACHIEVEMENTS] = {{{", ".join(str(p) for t, p in ach_c) or "0"}}};')
    P(f'static const uint8_t flag_init[{len(flag_init)}] = {{{c_bytes(flag_init)}}};')
    P(f'static const int16_t counter_init[N_COUNTERS] = {{{", ".join(map(str, counters_init))}}};')
    src = '\n'.join(lines) + '\n'
    ST.sizes = sizes
    ST.counts = dict(rooms=len(rooms_c), objects=len(objs_c), words=len(words_sorted), rules=len(rules_c),
                     events=len(events_c), flags=nflags, texts=len(TEXTS.items), tokens=len(table),
                     text_words=sum(len(re.findall(r"[A-Za-z']+", t.decode('latin-1'))) for t in TEXTS.items))
    if not check_only and out_path:
        with open(out_path, 'w', encoding='utf-8', newline='\n') as f:
            f.write(src)
    return src


def minute_of(raw, w):
    """A time of day as the minutes from the clock's start."""
    raw = raw.strip()
    if not ST.clock:
        D.error(w, f'{raw} is a time: it needs a clock (game: clock 21:00)')
        return 0
    return (parse_time(raw, w) - ST.clock[0]) % (24 * 60)


def c_bytes(b):
    return ','.join(map(str, b))


def pack_point(x, y):
    xi, yi = int(round(x * 2)), int(round(y * 2))
    v = (xi & 0xFFF) | (yi & 0xFFF) << 12
    return bytes([v & 255, (v >> 8) & 255, (v >> 16) & 255])


def shape_ink(pts):
    """The ink of a polyline as the vector device counts it (SPEC
    §vector.ink-cap), width 16 sixteenths, in square screen units."""
    w = 16
    total = 0
    if len(pts) == 1:
        pts = pts * 2
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        X0, Y0, X1, Y1 = x0 * 16, y0 * 16, x1 * 16, y1 * 16
        L = abs(X1 - X0) + abs(Y1 - Y0)
        stroke = (L + w) * (w + 48)
        g = (w + 48) // 2
        bx = max(0, min(X0, X1) - g), max(0, min(Y0, Y1) - g), min(10240, max(X0, X1) + g), min(7680, max(Y0, Y1) + g)
        box = max(0, bx[2] - bx[0]) * max(0, bx[3] - bx[1])
        total += -(-int(min(stroke, box)) // 256)
    return total


def check_story(rooms_c):
    # Reachability from the start through exits and effects.
    if not ST.rooms:
        return
    seen = set()
    todo = [ST.room_order[ST.start_index].id]
    while todo:
        r = todo.pop()
        if r in seen:
            continue
        seen.add(r)
        for e in ST.rooms[r].exits:
            if e['to'] and e['to'] in ST.rooms:
                todo.append(e['to'])
    for t in ST.go_targets:
        if t in ST.rooms and t not in seen:
            todo = [t]
            while todo:
                r = todo.pop()
                if r in seen:
                    continue
                seen.add(r)
                for e in ST.rooms[r].exits:
                    if e['to'] and e['to'] in ST.rooms:
                        todo.append(e['to'])
    for r in ST.room_order:
        if r.id not in seen:
            D.error(r.where, f'room "{r.id}" cannot be reached from the start (no exit or `go` leads there)')
        if not r.desc:
            D.warn(r.where, f'room "{r.id}" has no description')
    for o in ST.obj_order:
        if (o.loc is None or o.loc[0] in ('nowhere', 'none')) and o.id not in ST.moved:
            D.error(o.where, f'object "{o.id}" is never placed: give it `in <place>` or move it there in a rule')
        if o.loc and o.loc[0] in ST.objs:
            cont = ST.objs[o.loc[0]]
            if not cont.kinds & {'container', 'supporter'} and not cont.npc:
                D.error(o.loc[1], f'"{o.id}" is in "{cont.id}", which is not a container or supporter')
        if 'beam' in o.kinds and not o.kinds & {'light', 'switchable'}:
            D.warn(o.where, f'{o.id} has a beam but is not a switchable light')
    if 'win' not in ST.endings:
        D.warn('-', 'the story has no `win` effect: it cannot be won')


# --- Drawing (SVG, PNG) -----------------------------------------------------


def cond_eval_static(toks, flags):
    """Evaluates a layer's condition for --draw: flags given are set; for
    other conditions the default is false unless named with --flag (as the
    text, e.g. --flag "open door")."""
    parts, cur = [], []
    for t in toks:
        if t == 'and':
            parts.append(cur)
            cur = []
        else:
            cur.append(t)
    parts.append(cur)
    ok = True
    for p in parts:
        neg = False
        if p and p[0] == 'not':
            neg, p = True, p[1:]
        key = ' '.join(x if isinstance(x, str) else x[1] for x in p)
        v = key in flags
        if p and p[0] in ('closed', 'unlocked', 'off') and len(p) == 2:
            opp = {'closed': 'open', 'unlocked': 'locked', 'off': 'on'}[p[0]]
            v = f'{opp} {p[1]}' not in flags
        ok = ok and (v != neg)
    return ok


def collect_draw(target, flags, hidden, beam, objects):
    """The polylines to draw: list of (points, colour, style)."""
    out = []

    def add_art(art, ox, oy, beam_only=False):
        for L in art.layers:
            if L.cond and not cond_eval_static(L.cond[0], flags):
                continue
            style = 'solid'
            col = L.colour
            if L.hidden:
                revealed = L.reveal and L.reveal[0] in flags
                if not (hidden or revealed or beam):
                    continue
                style = 'hidden' if not revealed else 'revealed'
            if L.faint:
                col = tuple(c // 3 for c in col)
            for pts, w in L.shapes:
                out.append(([(x + ox, y + oy) for x, y in pts], col, style, L.hidden))
    if target in ST.rooms:
        r = ST.rooms[target]
        if r.art:
            add_art(r.art, 0, 0)
        if objects:
            for o in ST.obj_order:
                if o.loc and o.loc[0] == target and o.figure and o.at:
                    add_art(o.figure, o.at[0], o.at[1])
                if o.loc and o.loc[0] in ST.objs and ST.objs[o.loc[0]].loc and ST.objs[o.loc[0]].loc[0] == target and o.figure:
                    sup = ST.objs[o.loc[0]]
                    if 'supporter' in sup.kinds and sup.at:
                        t = sup.top or (0, -24)
                        add_art(o.figure, sup.at[0] + t[0], sup.at[1] + t[1])
        dark = r.dark
    elif target in ST.pictures:
        add_art(ST.pictures[target][1], 0, 0)
        dark = False
    elif target in ST.objs and ST.objs[target].figure:
        add_art(ST.objs[target].figure, SCENE_W / 2, SCENE_H / 2)
        dark = False
    else:
        raise StoryError(f'no room, picture or object with a figure called "{target}"' + closest(target, list(ST.rooms) + list(ST.pictures) + list(ST.objs)))
    return out, dark


def draw_svg(items, beam, path, dark):
    s = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="-10 -10 {SCENE_W + 20} {SCENE_H + 20}" width="{(SCENE_W + 20) * 2}" height="{(SCENE_H + 20) * 2}">',
         f'<rect x="-10" y="-10" width="{SCENE_W + 20}" height="{SCENE_H + 20}" fill="#000"/>',
         f'<rect x="0" y="0" width="{SCENE_W}" height="{SCENE_H}" fill="none" stroke="#333" stroke-width="1"/>']
    for x in range(0, SCENE_W + 1, 40):
        s.append(f'<text x="{x}" y="-2" font-size="6" fill="#555" text-anchor="middle">{x}</text>')
    for y in range(0, SCENE_H + 1, 40):
        s.append(f'<text x="-2" y="{y + 2}" font-size="6" fill="#555" text-anchor="end">{y}</text>')
    if beam:
        bx, by = beam
        s.append(f'<clipPath id="beam"><circle cx="{bx}" cy="{by}" r="{ST.beam}"/></clipPath>')
        s.append(f'<circle cx="{bx}" cy="{by}" r="{ST.beam}" fill="none" stroke="#554" stroke-width="0.7"/>')
    for pts, col, style, hidden in items:
        c = '#%02x%02x%02x' % col
        clip = ''
        if beam and (dark or (hidden and style == 'hidden')):
            clip = ' clip-path="url(#beam)"'
        dash = ' stroke-dasharray="3 2"' if style == 'hidden' and not beam else ''
        if len(pts) == 1:
            s.append(f'<circle cx="{pts[0][0]:g}" cy="{pts[0][1]:g}" r="0.9" fill="{c}"{clip}/>')
        else:
            p = ' '.join(f'{x:g},{y:g}' for x, y in pts)
            s.append(f'<polyline points="{p}" fill="none" stroke="{c}" stroke-width="1" stroke-linecap="round" stroke-linejoin="round"{dash}{clip}/>')
    s.append('</svg>')
    with open(path, 'w', encoding='utf-8', newline='\n') as f:
        f.write('\n'.join(s) + '\n')


def draw_png(items, beam, path, dark, scale=2):
    W, H = SCENE_W * scale, SCENE_H * scale
    buf = bytearray(W * H * 3)

    def plot(x, y, col):
        if 0 <= x < W and 0 <= y < H:
            if beam and clip_needed[0]:
                if (x / scale - beam[0]) ** 2 + (y / scale - beam[1]) ** 2 > ST.beam ** 2:
                    return
            i = (y * W + x) * 3
            for k in range(3):
                buf[i + k] = min(255, buf[i + k] + col[k])
    clip_needed = [False]
    for pts, col, style, hidden in items:
        clip_needed[0] = bool(beam) and (dark or (hidden and style == 'hidden'))
        if len(pts) == 1:
            pts = pts * 2
        for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
            n = int(max(abs(x1 - x0), abs(y1 - y0)) * scale) + 1
            for k in range(n + 1):
                t = k / n
                plot(int(round((x0 + (x1 - x0) * t) * scale)), int(round((y0 + (y1 - y0) * t) * scale)), col)
    raw = b''.join(b'\0' + bytes(buf[y * W * 3:(y + 1) * W * 3]) for y in range(H))

    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data) & 0xFFFFFFFF)
    png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', W, H, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw, 9)) + chunk(b'IEND', b'')
    with open(path, 'wb') as f:
        f.write(png)


# --- SVG import ---------------------------------------------------------------


def import_svg(path, layer, colour_name, scale, offset):
    import xml.etree.ElementTree as ET
    tree = ET.parse(path)
    root = tree.getroot()
    vb = root.get('viewBox')
    if scale is None:
        if vb:
            x0, y0, vw, vh = [float(v) for v in vb.replace(',', ' ').split()]
        else:
            x0 = y0 = 0
            vw = parse_len(root.get('width', str(SCENE_W)))
            vh = parse_len(root.get('height', str(SCENE_H)))
        scale = min(SCENE_W / vw, SCENE_H / vh)
        offset = offset or (-x0 * scale, -y0 * scale)
    offset = offset or (0, 0)
    shapes = []

    def walk(el, m):
        tag = el.tag.split('}')[-1]
        m2 = mat_mul(m, parse_transform(el.get('transform', '')))
        if tag in ('g', 'svg', 'a'):
            for c in el:
                walk(c, m2)
            return
        polys = []
        if tag == 'line':
            polys.append([(fl(el, 'x1'), fl(el, 'y1')), (fl(el, 'x2'), fl(el, 'y2'))])
        elif tag in ('polyline', 'polygon'):
            v = [float(x) for x in re.split(r'[\s,]+', el.get('points', '').strip()) if x]
            p = list(zip(v[0::2], v[1::2]))
            if tag == 'polygon' and p:
                p.append(p[0])
            polys.append(p)
        elif tag == 'rect':
            x, y, w, h = fl(el, 'x'), fl(el, 'y'), fl(el, 'width'), fl(el, 'height')
            polys.append([(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)])
        elif tag in ('circle', 'ellipse'):
            cx, cy = fl(el, 'cx'), fl(el, 'cy')
            rx = fl(el, 'r') if tag == 'circle' else fl(el, 'rx')
            ry = fl(el, 'r') if tag == 'circle' else fl(el, 'ry')
            polys.append([(cx + rx * math.cos(a / 24 * 2 * math.pi), cy + ry * math.sin(a / 24 * 2 * math.pi)) for a in range(25)])
        elif tag == 'path':
            polys += parse_path(el.get('d', ''), path)
        for p in polys:
            q = []
            for x, y in p:
                X, Y = apply(m2, x, y)
                q.append((X * scale + offset[0], Y * scale + offset[1]))
            if len(q) >= 2:
                shapes.append(q)
    walk(root, (1, 0, 0, 1, 0, 0))
    out = [f'    layer {layer} colour {colour_name}']
    for q in shapes:
        q = [(min(SCENE_W, max(0, x)), min(SCENE_H, max(0, y))) for x, y in q]
        out.append('      line ' + '  '.join(f'{fmt(x)} {fmt(y)}' for x, y in q))
    return '\n'.join(out)


def fmt(v):
    v = round(v * 2) / 2
    return str(int(v)) if v == int(v) else f'{v:.1f}'


def fl(el, name):
    return parse_len(el.get(name, '0'))


def parse_len(s):
    m = re.match(r'^\s*([-+]?[\d.]+(?:e[-+]?\d+)?)', s)
    return float(m.group(1)) if m else 0.0


def parse_transform(s):
    m = (1, 0, 0, 1, 0, 0)
    for name, args in re.findall(r'(\w+)\s*\(([^)]*)\)', s):
        a = [float(x) for x in re.split(r'[\s,]+', args.strip()) if x]
        if name == 'translate':
            t = (1, 0, 0, 1, a[0], a[1] if len(a) > 1 else 0)
        elif name == 'scale':
            t = (a[0], 0, 0, a[1] if len(a) > 1 else a[0], 0, 0)
        elif name == 'matrix':
            t = tuple(a)
        elif name == 'rotate':
            r = math.radians(a[0])
            t = (math.cos(r), math.sin(r), -math.sin(r), math.cos(r), 0, 0)
            if len(a) == 3:
                t = mat_mul(mat_mul((1, 0, 0, 1, a[1], a[2]), t), (1, 0, 0, 1, -a[1], -a[2]))
        else:
            continue
        m = mat_mul(m, t)
    return m


def mat_mul(m, n):
    a, b, c, d, e, f = m
    A, B, C2, D2, E2, F = n
    return (a * A + c * B, b * A + d * B, a * C2 + c * D2, b * C2 + d * D2, a * E2 + c * F + e, b * E2 + d * F + f)


def apply(m, x, y):
    a, b, c, d, e, f = m
    return a * x + c * y + e, b * x + d * y + f


def parse_path(d, path):
    toks = re.findall(r'[MmLlHhVvZzCcQqSsTtAa]|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?', d)
    polys, cur = [], []
    x = y = sx = sy = 0.0
    cmd = None
    i = 0

    def nums(n):
        nonlocal i
        v = [float(t) for t in toks[i:i + n]]
        i += n
        return v
    while i < len(toks):
        if re.match(r'[A-Za-z]', toks[i]):
            cmd = toks[i]
            i += 1
            if cmd in 'Zz':
                if cur:
                    cur.append((sx, sy))
                    polys.append(cur)
                    cur = []
                x, y = sx, sy
                continue
        if cmd is None:
            raise StoryError(f'{path}: a path must start with M')
        rel = cmd.islower()
        c = cmd.upper()
        if c == 'M':
            nx, ny = nums(2)
            if rel:
                nx, ny = nx + x, ny + y
            if cur:
                polys.append(cur)
            cur = [(nx, ny)]
            x, y, sx, sy = nx, ny, nx, ny
            cmd = 'l' if rel else 'L'
        elif c == 'L':
            nx, ny = nums(2)
            if rel:
                nx, ny = nx + x, ny + y
            cur.append((nx, ny))
            x, y = nx, ny
        elif c == 'H':
            (nx,) = nums(1)
            x = nx + x if rel else nx
            cur.append((x, y))
        elif c == 'V':
            (ny,) = nums(1)
            y = ny + y if rel else ny
            cur.append((x, y))
        elif c in 'CQ':
            n = 6 if c == 'C' else 4
            v = nums(n)
            if rel:
                v = [v[k] + (x if k % 2 == 0 else y) for k in range(n)]
            pts = [(x, y)] + list(zip(v[0::2], v[1::2]))
            for k in range(1, 9):
                t = k / 8
                cur.append(bezier(pts, t))
            x, y = pts[-1]
        else:
            raise StoryError(f'{path}: the path command {cmd} is not supported: use straight segments (or C and Q curves)')
    if cur:
        polys.append(cur)
    return polys


def bezier(pts, t):
    while len(pts) > 1:
        pts = [((1 - t) * a[0] + t * b[0], (1 - t) * a[1] + t * b[1]) for a, b in zip(pts, pts[1:])]
    return pts[0]


# --- Transcripts ------------------------------------------------------------


def transcript_events(path):
    """A transcript as a `terse run` script: each command typed (text events
    and Enter) 6 frames apart, a space pressed after each in case of a MORE
    prompt."""
    out = ['# from ' + os.path.basename(path)]
    f = 10
    with open(path, encoding='utf-8') as fh:
        for line in fh:
            s = line.rstrip('\n')
            if s.startswith('>'):
                cmd = s[1:].strip()
                out.append(f'{f} text {cmd}')
                out.append(f'{f + 1} press enter')
                f += 8
    return '\n'.join(out) + '\n'


# --- Main ------------------------------------------------------------------


def story_files(paths):
    files = []
    for p in paths:
        if os.path.isdir(p):
            files += sorted(os.path.join(p, f) for f in os.listdir(p) if f.endswith('.story'))
        else:
            files.append(p)
    if not files:
        raise StoryError('no .story files given')
    return files


def load(paths):
    lines = read_lines(story_files(paths))
    parse(lines)
    resolve()
    return lines


def report_diag():
    for w in D.warnings:
        print(w, file=sys.stderr)
    for e in D.errors:
        print(e, file=sys.stderr)


def main(argv):
    ap = argparse.ArgumentParser(description='The Low Bell story compiler (STORY.md).')
    ap.add_argument('story', nargs='*', help='a story directory or .story files')
    ap.add_argument('-o', '--output', help='the C header to write (default: story_data.h beside this script)')
    ap.add_argument('--check', action='store_true', help='check the story and print its sizes; write nothing')
    ap.add_argument('--draw', metavar='NAME', help='draw a room, picture or object figure to -o (.svg or .png)')
    ap.add_argument('--flag', action='append', default=[], help='with --draw: a flag (or condition, as "open door") taken as true')
    ap.add_argument('--hidden', action='store_true', help='with --draw: show hidden layers (dashed in SVG)')
    ap.add_argument('--beam', help='with --draw: the beam at X,Y (hidden layers and dark rooms seen through it)')
    ap.add_argument('--no-objects', action='store_true', help='with --draw: leave out the objects\' figures')
    ap.add_argument('--import-svg', metavar='FILE', help='print an SVG drawing as art lines')
    ap.add_argument('--layer', default='imported', help='with --import-svg: the layer name')
    ap.add_argument('--colour', default='white', help='with --import-svg: the layer colour')
    ap.add_argument('--scale', type=float, help='with --import-svg: units per SVG unit (default: fit the viewBox to 640 x 256)')
    ap.add_argument('--offset', help='with --import-svg: X,Y added after scaling')
    ap.add_argument('--events', metavar='TRANSCRIPT', help='print a transcript as a `terse run` events script')
    a = ap.parse_args(argv)
    try:
        if a.import_svg:
            off = tuple(float(v) for v in a.offset.split(',')) if a.offset else None
            print(import_svg(a.import_svg, a.layer, a.colour, a.scale, off))
            return 0
        if a.events:
            s = transcript_events(a.events)
            if a.output:
                with open(a.output, 'w', encoding='utf-8', newline='\n') as f:
                    f.write(s)
            else:
                sys.stdout.write(s)
            return 0
        here = os.path.dirname(os.path.abspath(__file__))
        paths = a.story or [os.path.join(here, 'story')]
        load(paths)
        if a.draw:
            if D.errors:
                report_diag()
                return 1
            beam = tuple(float(v) for v in a.beam.split(',')) if a.beam else None
            items, dark = collect_draw(a.draw, set(a.flag), a.hidden, beam, not a.no_objects)
            out = a.output or f'{a.draw}.svg'
            (draw_png if out.endswith('.png') else draw_svg)(items, beam, out, dark)
            report_diag()
            print(f'{out}: {a.draw}, {len(items)} shapes', file=sys.stderr)
            return 0
        out = None if a.check else (a.output or os.path.join(here, 'story_data.h'))
        src = emit(out, check_only=a.check)
        report_diag()
        if src is None:
            print(f'{len(D.errors)} error(s)', file=sys.stderr)
            return 1
        s, c = ST.sizes, ST.counts
        print(f'{c["rooms"]} rooms, {c["objects"]} objects, {c["words"]} words in the dictionary, {c["rules"]} rules, '
              f'{c["events"]} events, {c["flags"]} flags; {c["texts"]} texts, {c["text_words"]} words of text', file=sys.stderr)
        print(f'text {s["text_raw"]} bytes, compressed to {s["text"]} ({100 * s["text"] // max(1, s["text_raw"])}%, {c["tokens"]} pair tokens); '
              f'art {s["art"]}; rules and conditions {s["code"]}; dictionary {s["words"]}', file=sys.stderr)
        wr, wi = ST.worst_ink
        print(f'most ink: {wr}, {wi} square units ({100 * wi / INK_CAP:.1f}% of the cap)', file=sys.stderr)
        for where, n in TEXTS.long:
            print(f'{where}: note: this text takes {n} lines: it is shown with a MORE prompt', file=sys.stderr)
        if out:
            print(f'wrote {out}', file=sys.stderr)
        return 0
    except StoryError as e:
        report_diag()
        print(f'story.py: {e}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
