# Auto white balance

How the Color page's **Auto** finds the colour of the light in a clip and sets Temperature and Tint to take it back to neutral (build plan 7.2; the project plan's "Auto white balance"). Preliminary: the method is chosen on made-up pictures in a test; the comparison on the owner's facecam and game clips is still to do (section 3).

## 1. Problem

- **Facecams** under a warm lamp or a monitor's blue glow should come back to neutral: skin looks like skin and a white wall looks white.
- **Game footage** is often coloured on purpose (a purple neon level, a sepia flashback) and should be left almost as it is.
- It must never run by itself, write into the ordinary Temperature and Tint sliders so it can be fine-tuned or reset, have a strength, and look at several frames across the clip so one odd frame can't throw it off.
- It must be quick: a few frames, small, on a thread of its own.

## 2. Approaches

| Method | What it assumes | Where it goes wrong |
| --- | --- | --- |
| Grey world | the scene's colours average out to grey | a room with a big coloured wall, skin filling the frame |
| White patch (brightest near-neutral) | the brightest neutral things are white | a clipped lamp, a coloured light source in shot |
| Combined | each covers the other's failure | both wrong the same way |
| Learned (a small network estimating the light) | trained on many scenes | a model to ship, its licence, its size; no better on game footage, which no such model is trained on |

Every method only looks at pixels that can say something about the light: clipped ones (the sensor lost the colour), near-black ones (mostly noise) and strongly coloured ones (an object's own colour) are left out. Leaving out strongly coloured pixels also lets game footage through untouched: when too little is neutral, Auto says so and changes nothing.

A strong cast colours the greys themselves, so a fixed "strongly coloured" limit either keeps skin (too loose) or drops the greys under a strong cast (too strict). The estimate is therefore made three times, each on the picture as the last estimate would correct it, with the limit tightened from 0.5 to 0.35 to 0.25 ((most - least) / most).

## 3. Prototypes

So far, made-up pictures in `engine::white_balance`'s tests: a "room" of greys, skin, wood, a blue poster, a clipped lamp and shadows, lit by a warm light (1.25, 1, 0.7) or a neutral one, and a "neon" game picture of purples, teals and pinks.

| Picture | Grey world | White patch | Combined, three passes |
| --- | --- | --- | --- |
| Warm room | right within 0.12 of Temperature | right | right, Tint right within 0.1 |
| Neutral room | leans warm (skin counts as grey with one loose pass) | leans warm | left as it is (within 0.05) |
| Room with a pale green wall | leans green | leans less | leans less than grey world |
| Neon game picture | — | — | too little neutral: left as it is |

Still to do: the same on the owner's facecam clips (warm-lit, monitor-lit, daylight) and game clips with a look of their own, measured as how far a white or grey patch is from neutral after Auto, and a small learned model for comparison if one with a fitting licence is found.

## 4. Result (preliminary)

Combined grey world and white patch, three passes with a tightening limit, on 8 frames across the clip read 160 wide. Written into Temperature and Tint of the layer shown, times the strength (100 % at first). The "pick something white" eyedropper is still to come.
