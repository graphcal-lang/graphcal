---
icon: material/numeric-2-circle
---

# Step 2: Dimensions & Units

In this step, you'll add physical dimensions and units to your calculations, enabling compile-time dimensional analysis.

## Why Dimensions Matter

The [Mars Climate Orbiter](https://en.wikipedia.org/wiki/Mars_Climate_Orbiter) was lost because one team used imperial units while another used metric. Graphcal prevents this class of errors by checking dimensions at compile time.

## The Rocket Equation with Units

Create `rocket.gcl`:

```
dim Velocity = Length / Time;
dim Acceleration = Length / Time^2;

param dry_mass: Mass = 1200.0 kg;
param fuel_mass: Mass = 2800.0 kg;
param isp: Time = 320.0 s;
const node g0: Acceleration = 9.80665 m/s^2;

node v_exhaust: Velocity = @isp * @g0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

```bash
$ graphcal eval rocket.gcl
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 320 s
g0         = 9.80665 m/s^2
v_exhaust  = 3138.128 m/s
mass_ratio = 3.333333
delta_v    = 3778.220768 m/s
```

## Defining Dimensions

Graphcal has 7 built-in base dimensions: `Length`, `Time`, `Mass`, `Temperature`, `ElectricCurrent`, `Amount`, `LuminousIntensity`, plus `Angle`.

You define derived dimensions using algebraic expressions over base dimensions:

```
dim Velocity = Length / Time;
dim Acceleration = Length / Time^2;
dim Force = Mass * Length / Time^2;
dim Energy = Mass * Length^2 / Time^2;
```

## Using Units

The prelude provides common units. Attach a unit to a numeric literal:

```
param altitude: Length = 200.0 km;
param duration: Time = 3600.0 s;
const node speed_of_light: Velocity = 299792458.0 m/s;
```

### Available Prelude Units

| Dimension | Units |
|-----------|-------|
| Length | `m`, `km`, `cm`, `mm` |
| Time | `s`, `min`, `hour` |
| Mass | `kg`, `g` |
| Temperature | `K` |
| ElectricCurrent | `A` |
| Amount | `mol` |
| LuminousIntensity | `cd` |
| Angle | `rad`, `deg` |
| Force | `N`, `kN` |
| Energy | `J`, `kJ` |
| Power | `W`, `kW` |
| Pressure | `Pa`, `kPa`, `MPa` |
| Frequency | `Hz` |

## Defining Custom Units

You can define your own units:

```
unit mile: Length = 1609.344 m;
unit hour: Time = 3600.0 s;
unit mph: Velocity = 1.0 mile / hour;
```

## Unit Conversion

Use the `->` operator to convert between units of the same dimension:

```
param altitude: Length = 200.0 km;
node altitude_in_meters: Length = @altitude -> m;
```

The `->` operator only works when the source and target units share the same dimension. Attempting to convert `km` to `s` is a compile-time error.

## Dimension Checking

The compiler verifies that all expressions are dimensionally consistent. For example, this code:

```
param mass: Mass = 10.0 kg;
param length: Length = 5.0 m;
node bad: Mass = @mass + @length;  // ERROR!
```

produces a compile-time error because you cannot add `Mass` and `Length`.

## User-Defined Base Dimensions

You can define entirely new base dimensions for domain-specific quantities:

```
base dim Information;
base unit bit: Information;
unit byte: Information = 8.0 bit;
unit kB: Information = 1000.0 byte;

dim Bandwidth = Information / Time;

param storage: Information = 500.0 kB;
param rate: Bandwidth = 100.0 bit / s;
node transfer_time: Time = @storage / @rate;
```

A `base dim Information;` declaration creates a new base dimension.

## What You Learned

- **`dim`** declarations for derived and custom base dimensions
- **Unit annotations** on numeric literals (`1200.0 kg`)
- **`unit`** declarations for custom units
- **`->`** operator for unit conversion
- **Compile-time dimension checking** catches unit mismatches

## Next Step

In [Step 3](step3-structs-and-blocks.md), you'll learn to group related values with algebraic data types.
