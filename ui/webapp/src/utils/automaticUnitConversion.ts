// 自动转换时间单位
const autoTimeConversion = (originalTime: number) => {
    //（默认ns）
    // const units = [
    //     { unit: 'd', value: 86400 * 1e9 },
    //     { unit: 'h', value: 3600 * 1e9 },
    //     { unit: 'min', value: 60 * 1e9 },
    //     { unit: 's', value: 1e9 },
    //     { unit: 'ms', value: 1e6 },
    //     { unit: 'us', value: 1e3 },
    //     { unit: 'ns', value: 1 }
    // ];
    //默认us
    const units = [
        { unit: 'd', value: 24 * 3600 * 1e6 },
        { unit: 'h', value: 3600 * 1e6 },
        { unit: 'min', value: 60 * 1e6 },
        { unit: 's', value: 1e6 },
        { unit: 'ms', value: 1e3 },
        { unit: 'μs', value: 1 }
    ];
    if (originalTime == null || originalTime === 0) {
        return '0.00 μs';
    }

    const target: any = units.find(u => originalTime >= u.value) || { unit: 'μs', value: 1 };
    const value = (originalTime / target.value).toFixed(2);
    return `${value} ${target.unit}`;


    // return {
    //     value,
    //     unit: target.unit
    // }
}
//单位自动转换(默认B)
const autoUnitConversion = (unit: number) => {
    let units = ["B", "KB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"];
    let i = 0;
    if (unit) {
        while (unit >= 1024 && i < units.length - 1) {
            unit /= 1024;
            i++;
        }
        return `${Number(unit).toFixed(2)} ${units[i]}`;
    } else {
        return '0.00 B';
    }


}

export { autoTimeConversion, autoUnitConversion }