//! O gerador do `System.Random` do .NET quando construido com uma semente
//! explicita (`new Random(seed)`): o subtrativo de Knuth (Numerical Recipes
//! in C, 2a ed). O .NET mudou o algoritmo do construtor SEM semente a partir
//! do .NET 6, mas manteve este aqui para nao quebrar quem depende de
//! reproduzir a mesma sequencia a partir da mesma semente - e e exatamente
//! o caso do `TextTransforms.Mock`: a semente vem do StableHash do proprio
//! texto, e o /text mock precisa dar o MESMO resultado em Rust e em C#.
//!
//! Portado direto da fonte de referencia do .NET (`Random.cs`), inclusive a
//! aritmetica sem checagem de overflow (o C# roda em contexto `unchecked`
//! por padrao) - por isso todo `+`/`-` aqui usa `wrapping_*`.

const MBIG: i32 = i32::MAX;
const MSEED: i32 = 161803398;

pub struct LegacyRandom {
    seed_array: [i32; 56],
    inext: usize,
    inextp: usize,
}

impl LegacyRandom {
    pub fn new(seed: i32) -> Self {
        let mut seed_array = [0i32; 56];

        // Math.Abs(Int32.MinValue) lancaria OverflowException no C#; o
        // construtor trata esse caso a parte.
        let subtraction = if seed == i32::MIN {
            i32::MAX
        } else {
            seed.wrapping_abs()
        };

        let mut mj = MSEED.wrapping_sub(subtraction);
        seed_array[55] = mj;
        let mut mk: i32 = 1;
        for i in 1..55 {
            let ii = (21 * i) % 55;
            seed_array[ii] = mk;
            mk = mj.wrapping_sub(mk);
            if mk < 0 {
                mk = mk.wrapping_add(MBIG);
            }
            mj = seed_array[ii];
        }
        for _ in 1..5 {
            for i in 1..56 {
                seed_array[i] = seed_array[i].wrapping_sub(seed_array[1 + (i + 30) % 55]);
                if seed_array[i] < 0 {
                    seed_array[i] = seed_array[i].wrapping_add(MBIG);
                }
            }
        }

        Self {
            seed_array,
            inext: 0,
            inextp: 21,
        }
    }

    fn internal_sample(&mut self) -> i32 {
        let mut loc_inext = self.inext;
        let mut loc_inextp = self.inextp;

        loc_inext += 1;
        if loc_inext >= 56 {
            loc_inext = 1;
        }
        loc_inextp += 1;
        if loc_inextp >= 56 {
            loc_inextp = 1;
        }

        let mut ret_val = self.seed_array[loc_inext].wrapping_sub(self.seed_array[loc_inextp]);
        if ret_val == MBIG {
            ret_val -= 1;
        }
        if ret_val < 0 {
            ret_val = ret_val.wrapping_add(MBIG);
        }

        self.seed_array[loc_inext] = ret_val;
        self.inext = loc_inext;
        self.inextp = loc_inextp;
        ret_val
    }

    fn sample(&mut self) -> f64 {
        f64::from(self.internal_sample()) * (1.0 / f64::from(MBIG))
    }

    /// `Random.Next()`: qualquer valor entre 0 (inclusive) e Int32.MaxValue
    /// (exclusive). So os testes chamam isto hoje (o /text mock usa
    /// `next_max`), mas fica junto do resto da API do gerador por completude
    /// e porque os vetores de teste contra o C# precisam dele.
    #[allow(dead_code)]
    pub fn next(&mut self) -> i32 {
        self.internal_sample()
    }

    /// `Random.Next(maxValue)`: 0 (inclusive) a `max_value` (exclusive). Usado
    /// pelo /text mock com max_value=2.
    pub fn next_max(&mut self, max_value: i32) -> i32 {
        (self.sample() * f64::from(max_value)) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vetores tirados rodando `new Random(seed).Next()` de verdade num
    /// `dotnet run` fora do repo (o .csproj so chama o construtor do BCL,
    /// nada de proposito nosso), pra garantir bit a bit a mesma sequencia.
    #[test]
    fn bate_com_o_random_do_csharp() {
        let cases: &[(i32, &[i32])] = &[
            (
                0,
                &[
                    1559595546, 1755192844, 1649316166, 1198642031, 442452829, 1200195957,
                    1945678308, 949569752, 2099272109, 587775847,
                ],
            ),
            (
                1,
                &[
                    534011718, 237820880, 1002897798, 1657007234, 1412011072, 929393559, 760389092,
                    2026928803, 217468053, 1379662799,
                ],
            ),
            (
                -1,
                &[
                    534011718, 237820880, 1002897798, 1657007234, 1412011072, 929393559, 760389092,
                    2026928803, 217468053, 1379662799,
                ],
            ),
            (
                12345,
                &[
                    143337951, 150666398, 1663795458, 1097663221, 1712597933, 1776631026,
                    356393799, 1580828476, 558810388, 1086637143,
                ],
            ),
            (
                i32::MAX,
                &[
                    1559595546, 1755192844, 1649316172, 1198642031, 442452829, 1200195955,
                    1945678308, 949569752, 2099272109, 587775835,
                ],
            ),
            (
                i32::MIN,
                &[
                    1559595546, 1755192844, 1649316172, 1198642031, 442452829, 1200195955,
                    1945678308, 949569752, 2099272109, 587775835,
                ],
            ),
        ];

        for &(seed, expected) in cases {
            let mut random = LegacyRandom::new(seed);
            let got: Vec<i32> = (0..expected.len()).map(|_| random.next()).collect();
            assert_eq!(got, expected, "semente {seed}");
        }
    }

    /// O /text mock de verdade: StableHash do texto como semente, e
    /// `Next(2)` decidindo minuscula/maiuscula letra a letra.
    #[test]
    fn next_max_2_bate_com_o_mock_do_csharp() {
        let cases: &[(i32, &str)] = &[
            (0, "1 1 1 1 0 1 1 0 1 0 0 0 1 0 1 0 1 1 1 0"),
            (1, "0 0 0 1 1 0 0 1 0 1 0 0 0 1 1 1 0 1 1 1"),
            (12345, "0 0 1 1 1 1 0 1 0 1 0 0 0 0 0 1 0 1 1 0"),
            (42, "1 0 0 1 0 0 1 1 0 1 0 0 1 0 0 0 1 0 1 1"),
        ];

        for &(seed, expected) in cases {
            let mut random = LegacyRandom::new(seed);
            let got = (0..20)
                .map(|_| random.next_max(2).to_string())
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(got, expected, "semente {seed}");
        }
    }
}
