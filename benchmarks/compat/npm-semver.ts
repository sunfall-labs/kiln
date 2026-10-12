import semver from 'semver'; console.log(JSON.stringify([semver.valid('1.2.3'),semver.satisfies('1.2.3','^1.0.0')]));
