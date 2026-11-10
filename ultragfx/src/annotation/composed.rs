use crate::hle::{
    math::{identity, Mat4},
    mem::{GbiDataFormat, Rdram, RdramImage},
};

pub(super) struct ComposedView {
    pub matrix: Mat4,
    inverse: [[f64; 4]; 4],
}

impl ComposedView {
    pub fn new(words: &[u32; 16]) -> Option<Self> {
        let bytes: [u8; 64] = std::array::from_fn(|i| words[i / 4].to_be_bytes()[i % 4]);
        let matrix = RdramImage::new(&bytes)
            .read_matrix(0, GbiDataFormat::Fixed)
            .ok()?;
        Some(Self {
            matrix,
            inverse: inverse(matrix)?,
        })
    }

    pub fn model(&self, modelview: Mat4) -> Option<Mat4> {
        if modelview == self.matrix {
            return Some(identity());
        }
        let model: Mat4 = std::array::from_fn(|i| {
            std::array::from_fn(|j| {
                (0..4)
                    .map(|k| f64::from(modelview[i][k]) * self.inverse[k][j])
                    .sum::<f64>() as f32
            })
        });
        let unit_roundoff = f64::from(f32::EPSILON) / 2.0;
        let evaluation_error = 4.0 * f64::EPSILON;
        let evaluation_error = evaluation_error / (1.0 - evaluation_error);
        for (i, row) in modelview.iter().enumerate() {
            for (j, &value) in row.iter().enumerate() {
                let terms: [f64; 4] =
                    std::array::from_fn(|k| f64::from(model[i][k]) * f64::from(self.matrix[k][j]));
                let residual = (terms.iter().sum::<f64>() - f64::from(value)).abs();
                let magnitude = terms.iter().map(|v| v.abs()).sum::<f64>();
                let bound = 1.0 / 65536.0
                    + unit_roundoff / (1.0 - unit_roundoff) * (magnitude + f64::from(value).abs())
                    + evaluation_error * (magnitude + f64::from(value).abs());
                if !residual.is_finite() || residual > bound {
                    return None;
                }
            }
        }
        Some(model)
    }
}

fn inverse(matrix: Mat4) -> Option<[[f64; 4]; 4]> {
    let mut rows: [[f64; 8]; 4] = std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            if j < 4 {
                f64::from(matrix[i][j])
            } else {
                f64::from(u8::from(j - 4 == i))
            }
        })
    });
    for column in 0..4 {
        let pivot =
            (column..4).max_by(|&i, &j| rows[i][column].abs().total_cmp(&rows[j][column].abs()))?;
        rows.swap(column, pivot);
        let divisor = rows[column][column];
        if divisor == 0.0 {
            return None;
        }
        rows[column] = rows[column].map(|v| v / divisor);
        let pivot_row = rows[column];
        for (i, row) in rows.iter_mut().enumerate() {
            if i != column {
                let factor = row[column];
                for (value, pivot) in row.iter_mut().zip(pivot_row) {
                    *value -= factor * pivot;
                }
            }
        }
    }
    let result: [[f64; 4]; 4] = std::array::from_fn(|i| std::array::from_fn(|j| rows[i][j + 4]));
    // A right-inverse residual with infinity norm below one certifies nonsingularity.
    let error = 4.0 * f64::EPSILON;
    let error = error / (1.0 - error);
    for (i, row) in matrix.iter().enumerate() {
        let mut residual = 0.0;
        for (j, target) in identity()[i].into_iter().enumerate() {
            let terms: [f64; 4] = std::array::from_fn(|k| f64::from(row[k]) * result[k][j]);
            let target = f64::from(target);
            residual += (terms.iter().sum::<f64>() - target).abs()
                + error * (terms.iter().map(|v| v.abs()).sum::<f64>() + target);
        }
        if !residual.is_finite() || residual >= 1.0 {
            return None;
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependent_rows_are_singular_even_with_nonzero_elimination_pivots() {
        assert!(inverse([
            [-3.0, 10.0, -6.0, 6.0],
            [2.0, -10.0, -8.0, -5.0],
            [8.0, -9.0, -1.0, -10.0],
            [-1.0, 0.0, -14.0, 1.0],
        ])
        .is_none());
    }
}
