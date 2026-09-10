import { FullScreen, useFullScreenHandle } from "react-full-screen";
import { useState } from 'react';
import { Button } from 'antd';
import { Tracing } from 'aiprof-perfetto';
export const GPUTracing = () => {
    const handle = useFullScreenHandle();
    const [fullscreen, setFullscreen] = useState<boolean>(false)
    return (
        <>
            <FullScreen
                handle={handle}
                onChange={setFullscreen}
            >
                <div className={fullscreen ? 'fullscreen' : 'tracinggraph'} >
                    <div className='tracinggraphTitle'>
                    </div>
                    {!fullscreen ?
                        <Button type="primary"
                            style={{ margin: '0 0 20px 20px' }}
                            onClick={() => {
                                // 点击设置full为true，接着调用handle的enter方法，进入全屏模式
                                setFullscreen(true);
                                handle.enter();
                            }}
                        >进入全屏</Button> :
                        <Button type="primary"
                            style={{ margin: '0 0 20px 20px' }}
                            onClick={() => {
                                setFullscreen(false);
                                handle.exit();
                            }}
                        >退出全屏</Button>}
                    <div className='tracinggraphContent' style={{ backgroundColor: 'white', paddingBottom: '20px',height:'100vh', overflowY: 'auto' }} >
                        <Tracing />
                    </div>
                </div>
            </FullScreen>
        </>
    );
};
