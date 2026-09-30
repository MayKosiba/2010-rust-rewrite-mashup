"""A CS paint seed's pattern placement: Valve's uniform random stream
(`CUniformRandomStream`, Numerical Recipes' ran1) seeded with the paint
seed, rolled in the composite material's order: pattern offset x, offset y,
then rotation (`COMP_MAT_PROPERTY_MUTATOR_RANDOM_ROLL_INPUT_VARIABLES` in
weapons/paints/legacy/_shared_paint_generic.vcompmat)."""
import sys

IA, IM, IQ, IR, NTAB = 16807, 2147483647, 127773, 2836, 32
NDIV = 1 + (IM - 1) // NTAB
AM = 1.0 / IM
RNMX = 1.0 - 1.2e-7


def c_div(a, b):
    # C integer division truncates toward zero.
    q = abs(a) // abs(b)
    return q if (a >= 0) == (b >= 0) else -q


class UniformRandomStream:
    def __init__(self, seed):
        self.idum = seed if seed < 0 else -seed
        self.iy = 0
        self.iv = [0] * NTAB

    def _next(self):
        if self.idum <= 0 or not self.iy:
            self.idum = 1 if -self.idum < 1 else -self.idum
            for j in range(NTAB + 7, -1, -1):
                k = c_div(self.idum, IQ)
                self.idum = IA * (self.idum - k * IQ) - IR * k
                if self.idum < 0:
                    self.idum += IM
                if j < NTAB:
                    self.iv[j] = self.idum
            self.iy = self.iv[0]
        k = c_div(self.idum, IQ)
        self.idum = IA * (self.idum - k * IQ) - IR * k
        if self.idum < 0:
            self.idum += IM
        j = self.iy // NDIV
        if j >= NTAB or j < 0:
            j = (j % NTAB) & 0x7FFFFFFF
        self.iy = self.iv[j]
        self.iv[j] = self.idum
        return self.iy

    def random_float(self, low, high):
        fl = AM * self._next()
        if fl > RNMX:
            fl = RNMX
        return fl * (high - low) + low


def placement(seed):
    stream = UniformRandomStream(seed)
    x = stream.random_float(0.0, 1.0)
    y = stream.random_float(0.0, 1.0)
    rotation = stream.random_float(0.0, 360.0)
    return x, y, rotation


if __name__ == "__main__":
    for seed in map(int, sys.argv[1:] or ["661"]):
        x, y, r = placement(seed)
        print(f"seed {seed}: offset ({x:.6f}, {y:.6f}) rotation {r:.4f}")
